use crate::{Failure, Result};
use std::time::Duration;
use ureq::http::{
    header::{HeaderName, HeaderValue},
    Method,
};

pub const HELP: &str = r#"rill [OPTIONS] http[s]://HOST/PATH
One URL per invocation. Response body goes to stdout. No progress meter.
Redirects never downgrade HTTPS; origin changes allow only bodyless GET/HEAD
and permanently drop user headers. Buffered request input is limited to 8 MiB.
  -X, --request METHOD       Set request method
  -I, --head                 Fetch headers using HEAD
  -i, --include              Print response headers before the body
  -H, --header 'Name: val'   Add a header (repeatable)
  -d, --data DATA            Form POST; @file / @- strips CR, LF and NUL
      --data-raw DATA        Literal form data, including a leading @
      --data-binary DATA     Preserve every byte from @file or @-
      --json DATA            JSON POST with JSON Accept and Content-Type
      --max-filesize SIZE    Max body bytes; integer K/M/G/T/P suffixes; 0 = unlimited
      --atomic-output        Replace -o FILE only after a successful transfer
  -o, --output FILE          Write to FILE; use - for stdout
  -w, --write-out FORMAT     %{http_code}, %{response_code}, %%, \n, \r, \t
  -L, --location             Follow HTTP redirects (default limit 50)
      --max-redirs N         Set nonnegative redirect limit
  -m, --max-time SECONDS     Total transfer time, decimals allowed; 0 = unlimited
  -f, --fail                 HTTP status >= 400: suppress body and exit 22
      --fail-with-body       HTTP status >= 400: keep body and exit 22
  -s, --silent               Suppress transfer errors
  -S, --show-error           Show transfer errors even with -s
      --cacert FILE          Trust the roots in a PEM file for this request
  -h, --help                 Show this help
  -V, --version              Show version"#;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DataKind {
    Form,
    Raw,
    Binary,
    Json,
}

#[derive(Default)]
pub struct Options {
    pub url: Option<String>,
    pub method: Option<Method>,
    pub head: bool,
    pub include: bool,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub data: Vec<(DataKind, String)>,
    pub output: Option<String>,
    pub atomic_output: bool,
    pub max_filesize: Option<u64>,
    pub write_out: Option<String>,
    pub follow: bool,
    pub max_redirs: Option<u32>,
    pub timeout: Option<Duration>,
    pub fail: bool,
    pub fail_body: bool,
    pub silent: bool,
    pub show_error: bool,
    pub ca: Option<String>,
    pub help: bool,
    pub version: bool,
}

impl Options {
    pub fn parse(&mut self, args: Vec<String>) -> Result<()> {
        let mut args = args.into_iter();
        let mut positional = false;
        while let Some(arg) = args.next() {
            if self.help || self.version {
                return Ok(());
            }
            if !positional && arg == "--" {
                positional = true;
                continue;
            }
            if positional || !arg.starts_with('-') || arg == "-" {
                if self.url.replace(arg).is_some() {
                    return Err(Failure::usage("only one URL is supported"));
                }
                continue;
            }
            if let Some(long) = arg.strip_prefix("--") {
                let (name, inline) = long
                    .split_once('=')
                    .map_or((long, None), |(n, v)| (n, Some(v)));
                if self.flag(name) {
                    if inline.is_some() {
                        return Err(Failure::usage(format!("--{name} takes no value")));
                    }
                } else {
                    self.value(name, inline.map(str::to_owned), &mut args)?;
                }
            } else {
                let mut chars = arg[1..].chars();
                while let Some(short) = chars.next() {
                    let name = match short {
                        'I' => "head",
                        'i' => "include",
                        'L' => "location",
                        'f' => "fail",
                        's' => "silent",
                        'S' => "show-error",
                        'h' => "help",
                        'V' => "version",
                        'X' => "request",
                        'H' => "header",
                        'd' => "data",
                        'o' => "output",
                        'm' => "max-time",
                        'w' => "write-out",
                        _ => return Err(Failure::usage(format!("unsupported option -{short}"))),
                    };
                    if !self.flag(name) {
                        let rest = chars.as_str();
                        self.value(name, (!rest.is_empty()).then(|| rest.to_owned()), &mut args)?;
                        break;
                    }
                }
            }
        }
        if self.help || self.version {
            return Ok(());
        }
        if self.url.is_none() {
            return Err(Failure::usage("expected one HTTP(S) URL; see --help"));
        }
        if self.atomic_output && matches!(self.output.as_deref(), None | Some("-") | Some("")) {
            return Err(Failure::usage("--atomic-output needs -o FILE"));
        }
        if self.head && !self.data.is_empty() {
            return Err(Failure::usage("cannot combine --head with request data"));
        }
        let json = self
            .data
            .iter()
            .filter(|(k, _)| *k == DataKind::Json)
            .count();
        if json > 0 && json != self.data.len() {
            return Err(Failure::usage("cannot mix --json and form data"));
        }
        Ok(())
    }

    fn flag(&mut self, name: &str) -> bool {
        match name {
            "head" => self.head = true,
            "atomic-output" => self.atomic_output = true,
            "include" | "show-headers" => self.include = true,
            "location" => self.follow = true,
            "fail" => {
                self.fail = true;
                self.fail_body = false
            }
            "fail-with-body" => {
                self.fail = true;
                self.fail_body = true
            }
            "silent" => self.silent = true,
            "show-error" => self.show_error = true,
            "help" => self.help = true,
            "version" => self.version = true,
            _ => return false,
        }
        true
    }

    fn value(
        &mut self,
        name: &str,
        inline: Option<String>,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<()> {
        if !matches!(
            name,
            "request"
                | "header"
                | "data"
                | "data-raw"
                | "data-binary"
                | "json"
                | "output"
                | "write-out"
                | "max-redirs"
                | "max-filesize"
                | "max-time"
                | "cacert"
        ) {
            return Err(Failure::usage(format!("unsupported option --{name}")));
        }
        let v = inline
            .or_else(|| args.next())
            .ok_or_else(|| Failure::usage(format!("--{name} needs a value")))?;
        match name {
            "request" => {
                self.method = Some(Method::from_bytes(v.as_bytes()).map_err(Failure::usage)?)
            }
            "header" => {
                let (key, val) = v
                    .split_once(':')
                    .ok_or_else(|| Failure::usage("expected -H 'Name: value'"))?;
                let key = HeaderName::from_bytes(key.as_bytes()).map_err(Failure::usage)?;
                let val = val.trim_matches([' ', '\t']);
                if val.is_empty()
                    || matches!(
                        key.as_str(),
                        "host" | "content-length" | "transfer-encoding" | "proxy-authorization"
                    )
                {
                    return Err(Failure::usage(
                        "empty values and overrides of routing/framing headers are unsupported",
                    ));
                }
                self.headers
                    .push((key, HeaderValue::from_str(val).map_err(Failure::usage)?));
            }
            "data" => self.data.push((DataKind::Form, v)),
            "data-raw" => self.data.push((DataKind::Raw, v)),
            "data-binary" => self.data.push((DataKind::Binary, v)),
            "json" => self.data.push((DataKind::Json, v)),
            "output" => self.output = Some(v),
            "max-filesize" => self.max_filesize = parse_size(&v)?,
            "cacert" => self.ca = Some(v),
            "write-out" => {
                format_status(&v, 0)?;
                self.write_out = Some(v);
            }
            "max-redirs" => {
                self.max_redirs = Some(
                    v.parse()
                        .map_err(|_| Failure::usage("invalid redirect limit"))?,
                )
            }
            "max-time" => {
                let seconds = v
                    .parse::<f64>()
                    .map_err(|_| Failure::usage("invalid timeout"))?;
                let time = Duration::try_from_secs_f64(seconds)
                    .map_err(|_| Failure::usage("invalid timeout"))?;
                if time.as_secs() > u64::from(u32::MAX) {
                    return Err(Failure::usage("timeout too large"));
                }
                self.timeout = (!time.is_zero()).then_some(time);
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

pub fn format_status(mut format: &str, status: u16) -> Result<String> {
    let mut out = String::new();
    while !format.is_empty() {
        if let Some(tail) = format
            .strip_prefix("%{http_code}")
            .or_else(|| format.strip_prefix("%{response_code}"))
        {
            out.push_str(&format!("{status:03}"));
            format = tail;
        } else if let Some(tail) = format.strip_prefix("%%") {
            out.push('%');
            format = tail;
        } else if format.starts_with('%') {
            return Err(Failure::usage("unsupported --write-out variable"));
        } else if let Some(tail) = format.strip_prefix('\\') {
            let mut chars = tail.chars();
            out.push(match chars.next() {
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some('\\') => '\\',
                _ => return Err(Failure::usage("unsupported --write-out escape")),
            });
            format = chars.as_str();
        } else {
            let end = format.find(['%', '\\']).unwrap_or(format.len());
            out.push_str(&format[..end]);
            format = &format[end..];
        }
    }
    Ok(out)
}

fn parse_size(value: &str) -> Result<Option<u64>> {
    let err = || {
        Failure::usage(
            "invalid --max-filesize: expected an unsigned integer with optional K/M/G/T/P suffix",
        )
    };
    let end = value.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, suffix) = value.split_at(end);
    let number: u64 = digits.parse().map_err(|_| err())?;
    let scale: u64 = match suffix {
        "" => 1,
        "k" | "K" => 1 << 10,
        "m" | "M" => 1 << 20,
        "g" | "G" => 1 << 30,
        "t" | "T" => 1 << 40,
        "p" | "P" => 1 << 50,
        _ => return Err(err()),
    };
    let bytes = number.checked_mul(scale).ok_or_else(err)?;
    Ok((bytes != 0).then_some(bytes))
}

#[cfg(test)]
mod tests {
    use super::parse_size;
    #[test]
    fn size_units_and_boundaries() {
        for (input, expected) in [
            ("0", None),
            ("0K", None),
            ("1", Some(1)),
            ("0002k", Some(2048)),
            ("4M", Some(4 << 20)),
            ("1g", Some(1 << 30)),
            ("2T", Some(2 << 40)),
            ("1p", Some(1 << 50)),
            ("18446744073709551615", Some(u64::MAX)),
        ] {
            assert_eq!(parse_size(input).unwrap(), expected);
        }
        for input in [
            "",
            "+2",
            "-1",
            "1.5M",
            "3KB",
            " 4",
            "18446744073709551616",
            "16384P",
            "Ä",
        ] {
            assert!(parse_size(input).is_err());
        }
    }
}
