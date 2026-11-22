"""Compare a built rill with curl against temporary loopback HTTP and HTTPS servers."""
import argparse
from contextlib import ExitStack, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
import time


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "rill-test"
    def log_message(self, *args):
        pass

    def reply(self, status, data=b"", headers=()):
        self.send_response_only(status)
        self.send_header("Content-Length", str(len(data)))
        for key, val in headers:
            self.send_header(key, val)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def serve(self):
        try:
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            if self.path in self.server.routes:
                status, location = self.server.routes[self.path]
                self.reply(status, headers=[("Location", location)])
            elif self.path.startswith("/redir/"):
                _, _, code, destination = self.path.split("/", 3)
                target = {"rel":"../../echo", "self":self.path,
                          "peer":self.server.peer + "/echo", "ftp":"ftp://127.0.0.1/unread",
                          "slow":"/slow-step"}.get(destination, "/echo")
                self.reply(int(code), b"redirect body", [("Location", target)])
            elif self.path == "/slow-step":
                time.sleep(.3)
                self.reply(302, headers=[("Location", "/slow")])
            elif self.path == "/slow":
                time.sleep(.3)
                self.reply(200, b"done")
            elif self.path == "/slowbody":
                self.send_response_only(200)
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"a")
                self.wfile.flush()
                time.sleep(.45)
                self.wfile.write(b"b")
            elif self.path == "/partial":
                self.send_response_only(200)
                self.send_header("Content-Length", "99")
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(b"abc")
                self.close_connection = True
            elif self.path == "/chunked":
                self.send_response_only(200)
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                self.wfile.write(b"2\r\na\x00\r\n2\r\nb\xff\r\n0\r\n\r\n")
            elif self.path.startswith("/echo"):
                data = [self.command, self.path, body.hex(), self.headers.get("Content-Type"),
                        self.headers.get("Accept"), self.headers.get_all("X-Tag"),
                        self.headers.get("Authorization"), self.headers.get("Cookie")]
                self.reply(200, json.dumps(data, separators=(",", ":")).encode())
            elif self.path == "/missing":
                self.reply(404, b"missing")
            elif self.path == "/empty":
                self.reply(204)
            else:
                self.reply(200, b"ok\n", [("X-Reply", "one"), ("X-Reply", "two")])
        except (BrokenPipeError, ConnectionResetError):
            pass

    do_GET = do_HEAD = do_POST = do_PUT = do_PATCH = do_DELETE = serve


class QuietServer(ThreadingHTTPServer):
    def get_request(self):
        conn, address = super().get_request()
        with self.count_lock:
            self.accepted += 1
        if self.tls_context is not None:
            try:
                conn = self.tls_context.wrap_socket(conn, server_side=True)
            except OSError:
                conn.close()
                raise
        return conn, address

    def count(self):
        with self.count_lock:
            return self.accepted

    def handle_error(self, request, address):
        import sys
        if not isinstance(sys.exc_info()[1], (BrokenPipeError, ConnectionResetError)):
            super().handle_error(request, address)


@contextmanager
def server(ctx=None, handler=Handler):
    httpd = QuietServer(("127.0.0.1", 0), handler)
    httpd.routes = {}
    httpd.accepted = 0
    httpd.count_lock = threading.Lock()
    scheme = "https" if ctx else "http"
    httpd.tls_context = ctx
    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    try:
        yield httpd, f"{scheme}://127.0.0.1:{httpd.server_port}"
    finally:
        httpd.shutdown()
        httpd.server_close()
        t.join()


def response_view(blob):
    """Compare final body plus ordered status chain and multisets of header fields."""
    heads = []
    while blob.startswith(b"HTTP/"):
        block, blob = blob.split(b"\r\n\r\n", 1)
        line, *fields = block.split(b"\r\n")
        pairs = [(k.lower(), v.strip()) for k, v in (row.split(b":", 1) for row in fields)]
        heads.append((line, sorted(pairs)))
    return heads, blob


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rill", nargs="?", default="target/release/rill")
    binary = str(Path(parser.parse_args().rill).resolve())
    env = {k: v for k, v in os.environ.items() if "proxy" not in k.lower() and k not in
           {"CURL_CA_BUNDLE", "SSL_CERT_FILE", "SSL_CERT_DIR"}}
    checks = 0
    with tempfile.TemporaryDirectory() as temp, ExitStack() as stack:
        work = Path(temp)
        cert, key = work / "cert.pem", work / "key.pem"
        ca = work / "root.pem"
        def openssl(*args):
            subprocess.run(["openssl", *args], cwd=temp, check=True, capture_output=True)
        openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-subj", "/CN=rill-test-root", "-addext", "basicConstraints=critical,CA:TRUE",
                "-keyout", "ca.key", "-out", str(ca))
        openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=rill-test",
                "-keyout", str(key), "-out", "leaf.csr")
        (work / "leaf.ext").write_text("basicConstraints=critical,CA:FALSE\n"
                                       "extendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n")
        openssl("x509", "-req", "-in", "leaf.csr", "-CA", str(ca), "-CAkey", "ca.key",
                "-set_serial", "2", "-days", "1", "-extfile", "leaf.ext", "-out", str(cert))
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(cert, key)
        one, url = stack.enter_context(server())
        two, peer = stack.enter_context(server())
        secure, tls = stack.enter_context(server(ctx))
        one.peer = peer
        fixture = work / "body.dat"
        fixture.write_bytes(b"A\x00\r\nB\xff\n")
        output = work / "out"

        def invoke(command, flags, stdin):
            return subprocess.run(command + flags, input=stdin, capture_output=True, env=env,
                                  cwd=temp, timeout=8)

        def compare(label, flags, code=0, stdin=b"", headers=False, expected=None, outfile=False, diagnostic=None):
            nonlocal checks
            results = []
            for cmd in (["curl", "-q", "--http1.1"], [binary]):
                if outfile:
                    output.write_bytes(b"before")
                proc = invoke(cmd, flags, stdin)
                view = response_view(proc.stdout) if headers else proc.stdout
                results.append((proc.returncode, view, output.read_bytes() if outfile else None))
                assert proc.returncode == code, (label, cmd[0], proc.returncode, proc.stderr)
                if expected is not None:
                    assert proc.stdout == expected, (label, cmd[0], proc.stdout, expected)
                if diagnostic is not None:
                    assert bool(proc.stderr) == diagnostic, (label, cmd[0], proc.stderr)
            assert results[0] == results[1], (label, results)
            checks += 1
            print("ok", label)

        compare("GET body", [url], expected=b"ok\n")
        compare("empty 204", [url+"/empty"], expected=b"")
        compare("query with fragment", [url+"/echo?a=1%202#local"])
        compare("header casing and duplicates", ["-i", url], headers=True)
        compare("HEAD", ["-sSI", url], headers=True)
        compare("PATCH and repeated header", ["-XPATCH","-HX-Tag: A","-H","X-Tag: B",url+"/echo"])
        compare("repeated form fields", ["-da=b","-d","c=d",url+"/echo"])
        compare("form file filtering", ["-d","@"+str(fixture),url+"/echo"])
        compare("binary file", ["--data-binary","@"+str(fixture),url+"/echo"])
        compare("binary stdin", ["--data-binary","@-",url+"/echo"], stdin=fixture.read_bytes())
        compare("literal leading @", ["--data-raw","@literal",url+"/echo"])
        compare("empty first data", ["-d","","-d","b",url+"/echo"])
        compare("JSON", ["--json",'{"ok":true}',url+"/echo"])
        compare("JSON chunks", ["--json","{","--json","}",url+"/echo"])
        compare("override JSON headers", ["--json","{}","-H","Accept: text/plain","-H","Content-Type: text/plain",url+"/echo"])
        compare("binary download", ["-so",str(output),url+"/chunked"], outfile=True, expected=b"")
        compare("chunked stdout", [url+"/chunked"], expected=b"a\x00b\xff")
        compare("write-out", ["-so",str(output),"-w","%{http_code} %% %{response_code}\\n",url], outfile=True, expected=b"200 % 200\n")
        compare("default 404 status", [url+"/missing"], expected=b"missing")
        compare("fail leaves existing output", ["-fso",str(output),url+"/missing"], code=22, outfile=True, diagnostic=False)
        compare("silent plus show-error", ["-fsS",url+"/missing"], code=22, diagnostic=True, expected=b"")
        compare("fail-with-body", ["--fail-with-body","-w","/%{http_code}",url+"/missing"], code=22, expected=b"missing/404")
        compare("redirect disabled", [url+"/redir/302/rel"], expected=b"redirect body")
        for n in (301,302,303,307,308):
            compare(f"follow POST {n}", ["-sL","--data-binary","@"+str(fixture),url+f"/redir/{n}/rel"])
        compare("custom method on 302", ["-L","-X","PUT","-dpayload",url+"/redir/302/rel"])
        compare("all redirect header blocks", ["-iL",url+"/redir/302/rel"], headers=True)
        compare("included headers on HTTP failure", ["-sfi",url+"/missing"], code=22, headers=True)
        compare("JSON GET after 302", ["-L","--json","{}",url+"/redir/302/rel"])
        compare("redirected HEAD", ["-IL",url+"/redir/302/rel"], headers=True)
        compare("same-origin credentials retained", ["-L","-H","Authorization: Demo","-H","Cookie: a=b",url+"/redir/302/rel"])
        compare("cross-port credentials stripped", ["-L","-H","Authorization: Demo","-H","Cookie: a=b",url+"/redir/302/peer"])
        compare("redirect limit", ["-sL","--max-redirs","1","-w","%{http_code}",url+"/redir/302/self"], code=47)
        compare("time to headers", ["-sm0.13",url+"/slow"], code=28)
        compare("time to body", ["-sm0.2",url+"/slowbody"], code=28)
        compare("time budget across hops", ["-sLm0.45",url+"/redir/302/slow"], code=28)
        compare("incomplete response", ["-s",url+"/partial"], code=18)
        compare("TLS accepted private root", ["--cacert",str(ca),tls], expected=b"ok\n")
        compare("TLS rejects untrusted certificate", ["-s",tls], code=60)
        compare("TLS rejects mismatched host", ["-s","--cacert",str(ca),tls.replace("127.0.0.1","localhost")], code=60)
        compare("missing request file", ["--data-binary","@missing",url], code=26)
        compare("write error", ["-o",str(work/"missing"/"out"),url], code=23)

        def policy(label, flags, code, check=lambda p: True, stdin=b"", untouched=None):
            nonlocal checks
            before = untouched.count() if untouched is not None else 0
            proc = invoke([binary], flags, stdin)
            if untouched is not None:
                assert untouched.count() == before, (label, "unexpected TCP connection")
            assert proc.returncode == code and check(proc), (label, proc)
            checks += 1
            print("ok", label)
        policy("unsupported option rejected", ["--retry","3",url], 2)
        policy("multiple URLs rejected", [url,url], 2)
        policy("unsupported scheme", ["file:///example"], 1)
        policy("redirect protocol restricted", ["-L",url+"/redir/302/ftp"], 1)
        policy("custom header withheld across origins", ["-LH","X-Tag: private",url+"/redir/302/peer"],
               0, lambda p: json.loads(p.stdout)[5] is None)
        policy("explicitly invalid PEM", ["--cacert",str(fixture),tls], 77)
        policy("help available", ["--help"], 0, lambda p: b"--json" in p.stdout)
        # Test only the release executable against loopback peers, with disposable data.
        one.routes["/away"] = (302, peer + "/return")
        two.routes["/return"] = (302, url + "/echo")
        secure.routes["/down"] = (302, peer + "/echo")
        one.routes["/up"] = (302, tls + "/down")
        one.routes["/upgrade"] = (302, tls + "/echo")
        policy("foreign hop permanently drops user headers", [
            "-L","-H","Authorization: test","-H","Cookie: test","-H","X-Tag: test",url+"/away"
        ], 0, lambda p: json.loads(p.stdout)[5:] == [None,None,None])
        for status in (307, 308):
            policy(f"cross-origin {status} never connects with body", [
                "-L","--json","{}",url+f"/redir/{status}/peer"
            ], 1, untouched=two)
        policy("empty explicit body still protected", ["-L","-d","",url+"/redir/307/peer"],
               1, untouched=two)
        policy("custom GET with body protected", ["-L","-XGET","-d","test",url+"/redir/308/peer"],
               1, untouched=two)
        policy("bodyless DELETE cannot cross origin", ["-L","-XDELETE",url+"/redir/303/peer"],
               1, untouched=two)
        policy("foreign 302 POST becomes bodyless GET", ["-L","-d","test",url+"/redir/302/peer"],
               0, lambda p: json.loads(p.stdout)[:3] == ["GET","/echo",""])
        policy("cross-origin HEAD is allowed", ["-IL",url+"/redir/307/peer"], 0,
               lambda p: len(response_view(p.stdout)[0]) == 2)
        policy("HTTPS downgrade before target connection", ["-L","--cacert",str(ca),tls+"/down"],
               1, untouched=two)
        policy("upgrade remains protected on further hops", ["-L","--cacert",str(ca),url+"/up"],
               1, untouched=two)
        policy("HTTP upgrade GET allowed", ["-L","--cacert",str(ca),url+"/upgrade"], 0,
               lambda p: json.loads(p.stdout)[0] == "GET")
        policy("zero redirect budget", ["-L","--max-redirs","0",url+"/redir/302/peer"],
               47, untouched=two)
        policy("exhausted limit has priority over target parsing", [
            "-L","--max-redirs","0",url+"/redir/302/ftp"
        ], 47)
        cap = 8 * 1024 * 1024
        large = work / "large.dat"
        large.write_bytes(b"x" * (cap+1))
        policy("buffer limit inclusive", ["--data-binary","@-",url], 0,
               lambda p: p.stdout == b"ok\n", stdin=b"x"*cap)
        policy("stdin over cap", ["--data-binary","@-",url], 26, stdin=b"x"*(cap+1), untouched=one)
        policy("filtered bytes still consume budget", ["-d","@-",url], 26,
               stdin=b"\n"*(cap+1), untouched=one)
        policy("separator charged across fields", ["-da","--data-binary","@-",url], 26,
               stdin=b"x"*(cap-1), untouched=one)
        policy("regular file still streams beyond cap", ["--data-binary","@"+str(large),url], 0,
               lambda p: p.stdout == b"ok\n")
        policy("oversized CA bundle rejected before TLS", ["--cacert",str(large),tls], 77, untouched=secure)
    print(f"PASS {checks} cases")


if __name__ == "__main__":
    main()
