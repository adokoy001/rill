use crate::{
    cli::{DataKind, Options},
    output::{write_error, Output},
    policy::{take_prefix, Chain, Denied, Redirect},
    upload::Payload,
    Failure, Result,
};
use fluent_uri::{Uri, UriRef};
use std::{
    fs::File,
    io::{self, IsTerminal, Read, Write},
    time::Instant,
};
use ureq::{
    config::AutoHeaderValue,
    http::{header, Method, Request, Response},
    tls::{parse_pem, PemItem, RootCerts, TlsConfig},
    Agent,
};

fn make_agent(opts: &Options) -> Result<Agent> {
    let mut config = Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .accept_encoding(AutoHeaderValue::None)
        .allow_non_standard_methods(true)
        .user_agent(concat!("rill/", env!("CARGO_PKG_VERSION")));
    if let Some(path) = &opts.ca {
        const MAX_PEM_BYTES: u64 = 4 * 1024 * 1024;
        let mut pem = Vec::new();
        File::open(path)
            .and_then(|f| f.take(MAX_PEM_BYTES + 1).read_to_end(&mut pem))
            .map_err(|e| Failure::new(77, e))?;
        if pem.len() as u64 > MAX_PEM_BYTES {
            return Err(Failure::new(77, "CA bundle exceeds 4 MiB"));
        }
        let mut roots = Vec::new();
        for item in parse_pem(&pem) {
            if let PemItem::Certificate(cert) = item.map_err(|e| Failure::new(77, e))? {
                roots.push(cert);
            }
        }
        if roots.is_empty() {
            return Err(Failure::new(77, "CA file contains no certificates"));
        }
        config = config.tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::new_with_certs(&roots))
                .build(),
        );
    }
    Ok(config.build().into())
}

#[derive(PartialEq, Eq)]
struct Origin {
    https: bool,
    host: String,
    port: u16,
}

struct Target {
    reference: Uri<String>,
    wire: ureq::http::Uri,
    origin: Origin,
}

impl Target {
    fn new(url: Uri<String>) -> Result<Self> {
        let scheme = url.scheme().as_str().to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return Err(Failure::new(1, "only http and https are supported")),
        };
        let auth = url
            .authority()
            .ok_or_else(|| Failure::new(3, "URL needs a host"))?;
        if auth.host().is_empty() || auth.userinfo().is_some() {
            return Err(Failure::new(3, "empty host or URL userinfo is unsupported"));
        }
        let port = auth
            .port_to_u16()
            .map_err(|e| Failure::new(3, e))?
            .unwrap_or(default_port);
        let wire = url
            .as_str()
            .parse::<ureq::http::Uri>()
            .map_err(|e| Failure::new(3, e))?;
        let host = wire.host().ok_or_else(|| Failure::new(3, "missing host"))?;
        if !host.eq_ignore_ascii_case(auth.host())
            || wire.port_u16().unwrap_or(default_port) != port
            || !wire
                .scheme_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(&scheme))
        {
            return Err(Failure::new(3, "ambiguous URL authority"));
        }
        let origin = Origin {
            https: scheme == "https",
            host: host.to_ascii_lowercase(),
            port,
        };
        Ok(Self {
            reference: url,
            wire,
            origin,
        })
    }
    fn join(&self, location: &header::HeaderValue) -> Result<Self> {
        let reference = UriRef::parse(location.to_str().map_err(|e| Failure::new(3, e))?)
            .map_err(|e| Failure::new(3, e))?;
        let resolved = reference
            .resolve_against(&self.reference)
            .map_err(|e| Failure::new(3, e))?;
        Self::new(resolved.strip_fragment().to_owned())
    }
}

fn tls_code(error: &rustls::Error) -> u8 {
    match error {
        rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented => 60,
        _ => 35,
    }
}

fn network_error(error: ureq::Error, during_body: bool) -> Failure {
    use io::ErrorKind as K;
    let code = match &error {
        ureq::Error::HostNotFound => 6,
        ureq::Error::ConnectionFailed => 7,
        ureq::Error::Timeout(_) => 28,
        ureq::Error::BadUri(_) => 3,
        ureq::Error::Rustls(e) => tls_code(e),
        ureq::Error::Tls(_) => 35,
        ureq::Error::Io(e) => {
            if let Some(tls) = e.get_ref().and_then(|s| s.downcast_ref::<rustls::Error>()) {
                tls_code(tls)
            } else {
                match e.kind() {
                    K::TimedOut => 28,
                    K::ConnectionRefused | K::NetworkUnreachable | K::HostUnreachable => 7,
                    K::UnexpectedEof if during_body => 18,
                    K::UnexpectedEof => 52,
                    _ => 56,
                }
            }
        }
        _ => 56,
    };
    Failure::new(code, error)
}

fn write_headers(response: &Response<ureq::Body>, out: &mut dyn Write) -> io::Result<()> {
    let status = response.status();
    write!(
        out,
        "{:?} {} {}\r\n",
        response.version(),
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )?;
    for (name, value) in response.headers() {
        write!(out, "{name}: ")?;
        out.write_all(value.as_bytes())?;
        out.write_all(b"\r\n")?;
    }
    out.write_all(b"\r\n")
}

pub fn run(opts: &Options, last_status: &mut u16, output: &mut Output) -> Result<()> {
    let raw = opts.url.as_deref().unwrap();
    let url = Uri::parse(raw)
        .map_err(|e| Failure::new(3, e))?
        .strip_fragment()
        .to_owned();
    let mut target = Target::new(url)?;
    let mut payload = Payload::load(opts)?;
    let json = opts.data.first().is_some_and(|(k, _)| *k == DataKind::Json);
    let agent = make_agent(opts)?;
    let start = Instant::now();
    let mut chain = Chain::start(
        target.origin.https,
        !matches!(payload, Payload::Empty(_)),
        opts.max_redirs.unwrap_or(50),
    );
    let explicit_other = opts
        .method
        .as_ref()
        .is_some_and(|m| m != Method::GET && m != Method::HEAD);
    loop {
        let send_data = chain.body();
        let default_method = if opts.head {
            Method::HEAD
        } else if send_data {
            Method::POST
        } else {
            Method::GET
        };
        let method = opts.method.as_ref().unwrap_or(&default_method);
        let mut empty = Payload::Empty(());
        let permitted_payload = if send_data { &mut payload } else { &mut empty };
        let body = permitted_payload.as_body()?;
        let mut request = Request::builder()
            .method(method)
            .uri(target.wire.clone())
            .body(body)
            .map_err(|e| Failure::new(3, e))?;
        let headers = request.headers_mut();
        if chain.user_headers() {
            for (name, value) in &opts.headers {
                headers.append(name, value.clone());
            }
        }
        if json {
            headers
                .entry(header::ACCEPT)
                .or_insert(header::HeaderValue::from_static("application/json"));
        }
        if send_data || json {
            headers
                .entry(header::CONTENT_TYPE)
                .or_insert(header::HeaderValue::from_static(if json {
                    "application/json"
                } else {
                    "application/x-www-form-urlencoded"
                }));
        }
        let remaining = opts
            .timeout
            .map(|budget| {
                budget
                    .checked_sub(start.elapsed())
                    .filter(|t| !t.is_zero())
                    .ok_or_else(|| Failure::new(28, "transfer timed out"))
            })
            .transpose()?;
        let request = agent
            .configure_request(request)
            .timeout_global(remaining)
            .build();
        let mut response = agent.run(request).map_err(|e| network_error(e, false))?;
        let status = response.status().as_u16();
        *last_status = status;
        if opts.include || opts.head {
            write_headers(&response, output.writer()?).map_err(write_error)?;
        }
        if let (true, Some(mode), Some(location)) = (
            opts.follow,
            Redirect::from_status(status),
            response
                .headers()
                .get(header::LOCATION)
                .filter(|v| !v.is_empty()),
        ) {
            // Keep curl's limit error ahead of parsing a target that cannot be followed.
            if chain.exhausted() {
                return Err(Failure::new(47, "redirect limit exceeded"));
            }
            let next = target.join(location)?;
            chain = chain
                .follow(
                    mode,
                    target.origin == next.origin,
                    next.origin.https,
                    explicit_other,
                )
                .map_err(|denied| match denied {
                    Denied::Limit => Failure::new(47, "redirect limit exceeded"),
                    Denied::Downgrade => Failure::new(1, "blocked HTTPS to HTTP redirect"),
                    Denied::CrossOriginWrite => Failure::new(
                        1,
                        "blocked cross-origin request with a body or non-GET/HEAD method",
                    ),
                })?;
            target = next;
            continue;
        }
        let failed = opts.fail && status >= 400;
        if !(opts.head || failed && !opts.fail_body) {
            copy_body(response.body_mut(), opts, output)?;
        }
        output.flush()?;
        if failed {
            return Err(Failure::new(22, format!("HTTP {status}")));
        }
        return Ok(());
    }
}

fn copy_body(body: &mut ureq::Body, opts: &Options, output: &mut Output) -> Result<()> {
    let too_large = || Failure::new(63, "response body exceeds --max-filesize");
    let mut remaining = opts.max_filesize;
    if body
        .content_length()
        .zip(remaining)
        .is_some_and(|(len, limit)| len > limit)
    {
        return Err(too_large());
    }
    let protect_terminal = opts.output.is_none() && io::stdout().is_terminal();
    let mut reader = body.as_reader();
    let mut buf = [0u8; 16 * 1024];
    let out = output.writer()?;
    let mut first = true;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| network_error(e.into(), true))?;
        if n == 0 {
            return Ok(());
        }
        let count = remaining.as_mut().map_or(n, |r| take_prefix(r, n));
        if first && protect_terminal && buf[..count].contains(&0) {
            return Err(Failure::new(
                23,
                "binary output on a terminal; choose -o FILE or -o -",
            ));
        }
        first = false;
        out.write_all(&buf[..count]).map_err(write_error)?;
        if count < n {
            return Err(too_large());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(raw: &str) -> Result<Target> {
        Target::new(Uri::parse(raw).unwrap().to_owned())
    }

    #[test]
    fn origin_uses_effective_port_and_ascii_case() {
        let a = target("https://EXAMPLE.com:443/a").unwrap();
        let b = target("https://example.com/b?q=2").unwrap();
        assert!(a.origin == b.origin);
        assert!(a.origin != target("https://example.com:8443/a").unwrap().origin);
        assert!(a.origin != target("http://example.com:443/a").unwrap().origin);
        assert!(a.origin != target("https://sub.example.com/a").unwrap().origin);
    }

    #[test]
    fn ipv6_relative_reference_preserves_origin() {
        let a = target("http://[::1]:8080/a/b").unwrap();
        let b = a
            .join(&header::HeaderValue::from_static("../c#fragment"))
            .unwrap();
        assert!(a.origin == b.origin);
        assert_eq!(b.wire.to_string(), "http://[::1]:8080/c");
    }

    #[test]
    fn invalid_destinations_are_rejected() {
        for url in [
            "ftp://example.com/a",
            "https://user@example.com/",
            "http://example.com:65536/a",
        ] {
            assert!(target(url).is_err());
        }
    }
}
