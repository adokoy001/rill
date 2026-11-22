"""Exercise download bounds and atomic publication through the actual CLI."""
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
from threading import Event
import time

from compare import Handler, server


class DownloadHandler(Handler):
    def do_GET(self):
        if self.path == "/unframed":
            self.send_response_only(200)
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(b"abcde")
            self.close_connection = True
        elif self.path.startswith("/gate/"):
            tag = self.path[-1]
            data = tag.encode() * 65536
            self.send_response_only(200)
            self.send_header("Content-Length", str(len(data) + 1))
            self.end_headers()
            self.wfile.write(data)
            self.wfile.flush()
            if self.server.gates[tag].wait(8):
                self.wfile.write(b".")
            else:
                self.close_connection = True
        else:
            super().do_GET()


def main():
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/rill").resolve())
    env = {k:v for k, v in os.environ.items() if "proxy" not in k.lower()}
    checks = 0
    with tempfile.TemporaryDirectory() as tmp, server(handler=DownloadHandler) as (httpd, base):
        work = Path(tmp)
        dest = work / "result"
        def run(flags, cmd=None, stdout=subprocess.PIPE):
            return subprocess.run((cmd or [binary]) + flags, env=env, cwd=work, stdout=stdout,
                                  stderr=subprocess.PIPE, timeout=10)
        def ok(label):
            nonlocal checks
            checks += 1
            print("ok", label)
        def parity(label, flags, expected, code):
            for cmd in (["curl","-q","--http1.1"], [binary]):
                proc = run(flags, cmd)
                assert (proc.returncode,proc.stdout) == (code,expected), (label,cmd,proc)
            ok(label)
        for label, path, limit, expected, code in [
            ("known/exact","/", "3", b"ok\n",0), ("known/oversized","/", "2", b"",63),
            ("known/suffix","/", "1K", b"ok\n",0), ("chunked/exact","/chunked","4",b"a\x00b\xff",0),
            ("chunked/oversized","/chunked","3",b"a\x00b",63),
            ("close/exact","/unframed","5",b"abcde",0),
            ("close/oversized","/unframed","4",b"abcd",63),
        ]:
            parity(label, ["-s","--max-filesize",limit,base+path],expected,code)
        def case(label, flags, code, output=b""):
            proc=run(flags)
            assert (proc.returncode,proc.stdout)==(code,output),(label,proc)
            ok(label)
        case("zero resets limit", ["--max-filesize","1","--max-filesize","0",base],0,b"ok\n")
        case("HEAD ignores hypothetical body", ["-Io","headers","--max-filesize","1",base],0)
        assert b"HTTP/1.1 200" in (work/"headers").read_bytes()
        case("write-out independent of limit", ["-sw","/%{http_code}","--max-filesize","2",base],63,b"/200")
        case("reject invalid size", ["--max-filesize","-2",base],2)
        case("atomic needs file", ["--atomic-output",base],2)
        case("atomic refuses stdout", ["--atomic-output","-o","-",base],2)
        httpd.peer = base
        def atomic(label, path, extra, code, want, existing=b"before", stdout=b""):
            dest.unlink(missing_ok=True)
            if existing is not None:
                dest.write_bytes(existing)
            proc = run(["--atomic-output","-o","result",*extra,base+path])
            actual = dest.read_bytes() if dest.exists() else None
            assert (proc.returncode,proc.stdout,actual) == (code,stdout,want),(label,proc,actual)
            assert not list(work.glob(".rill-*")), label
            ok(label)
        atomic("publish new file", "/",[],0,b"ok\n",None)
        atomic("publish over existing file", "/chunked",["--max-filesize","4"],0,b"a\x00b\xff")
        atomic("publish empty file", "/empty",[],0,b"")
        atomic("HTTP error is data by default","/missing",[],0,b"missing")
        atomic("fail keeps old file","/missing",["-f"],22,b"before")
        atomic("fail-with-body rolls back","/missing",["--fail-with-body"],22,b"before")
        atomic("partial response rolls back","/partial",[],18,b"before")
        atomic("timeout rolls back","/slowbody",["-m",".15"],28,b"before")
        atomic("headers and body roll back","/chunked",["-i","--max-filesize","2"],63,b"before")
        atomic("known oversize keeps destination absent","/",["--max-filesize","1"],63,None,None)
        atomic("unknown oversize keeps destination absent","/unframed",["--max-filesize","2"],63,None,None)
        atomic("redirect rejection rolls back headers","/redir/302/self",["-iL","--max-redirs","0"],47,b"before")
        atomic("write-out is separate stream","/",["-w","%{http_code}"],0,b"ok\n",stdout=b"200")
        if os.name == "posix":
            assert stat.S_IMODE(dest.stat().st_mode) & 0o077 == 0
            ok("private file permissions")
            alias = work/"alias"
            alias.symlink_to(dest)
            case("symlink rejected", ["--atomic-output","-o","alias",base+"/empty"],23)
            assert alias.is_symlink() and dest.read_bytes()==b"ok\n"
            fifo = work/"pipe"
            os.mkfifo(fifo)
            case("special file rejected", ["--atomic-output","-o","pipe",base],23)
            assert stat.S_ISFIFO(fifo.stat().st_mode)
        if Path("/dev/full").exists():
            dest.write_bytes(b"old")
            with open("/dev/full","wb",buffering=0) as full:
                proc = run(["--atomic-output","-o","result","-w","%{http_code}",base],stdout=full)
            assert proc.returncode == 23 and dest.read_bytes() == b"old" and not list(work.glob(".rill-*"))
            ok("failed write-out rolls back")
        def wait_staged(count):
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                staged = list(work.glob(".rill-*"))
                if len(staged) == count and all(f.stat().st_size > 0 for f in staged):
                    return
                time.sleep(.01)
            raise AssertionError("expected partial data in each staged file")
        httpd.gates = {tag:Event() for tag in ["a","b"]}
        dest.write_bytes(b"old")
        children=[]
        try:
            for tag in httpd.gates:
                children.append(subprocess.Popen([binary,"--atomic-output","-o","result",base+"/gate/"+tag],
                                                 cwd=work,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE))
            wait_staged(2)
            assert dest.read_bytes()==b"old"
            for tag,child in zip(httpd.gates,children):
                httpd.gates[tag].set()
                _,err=child.communicate(timeout=5)
                assert child.returncode==0,err
                assert dest.read_bytes()==tag.encode()*65536+b"."
            assert not list(work.glob(".rill-*"))
            ok("concurrent readers see only old or completed versions")
        finally:
            for gate in httpd.gates.values():
                gate.set()
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.wait()
        if os.name == "posix":
            for mode in ("target changed to symlink", "process killed before commit"):
                httpd.gates["a"] = Event()
                dest.unlink(missing_ok=True)
                dest.write_bytes(b"old")
                child = subprocess.Popen([binary,"--atomic-output","-o","result",base+"/gate/a"],
                                         cwd=work,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                try:
                    wait_staged(1)
                    if mode == "target changed to symlink":
                        other = work/"untouched"
                        other.write_bytes(b"other")
                        dest.unlink()
                        dest.symlink_to(other)
                        httpd.gates["a"].set()
                        child.communicate(timeout=5)
                        assert child.returncode == 23 and dest.is_symlink()
                        assert other.read_bytes() == b"other" and not list(work.glob(".rill-*"))
                    else:
                        child.kill()
                        child.communicate(timeout=5)
                        assert child.returncode < 0 and dest.read_bytes() == b"old"
                    ok(mode)
                finally:
                    httpd.gates["a"].set()
                    if child.poll() is None:
                        child.kill()
                    child.wait()
    print(f"PASS {checks} download cases")


if __name__ == "__main__":
    main()
