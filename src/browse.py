#!/usr/bin/env python3
"""Small dependency browser used by `knight -t browse`."""

import argparse
from html import escape
from http.server import BaseHTTPRequestHandler, HTTPServer
import socketserver
import subprocess
import sys
from urllib.parse import quote, unquote
import webbrowser


def query(target):
    command = [args.ninja_command, "-f", args.f, "-t", "query", target]
    return subprocess.run(command, capture_output=True, text=True)


def page(target, result):
    if result.returncode:
        body = "<h1><code>%s</code></h1>" % escape(result.stderr)
    else:
        lines = result.stdout.splitlines()
        links = []
        for line in lines[1:]:
            value = line.strip().removeprefix("| ").removeprefix("|| ")
            if not value or value.endswith(":") or value.startswith("input: "):
                continue
            links.append(
                '<li><a href="?%s"><code>%s</code></a></li>'
                % (quote(value), escape(value))
            )
        body = "<h1><code>%s</code></h1><ul>%s</ul>" % (
            escape(target),
            "".join(links),
        )
    return ("<!doctype html><meta charset=utf-8><title>Knight graph</title>"
            "<style>body{font:14px system-ui;margin:3rem;max-width:70rem}"
            "li{margin:.35rem 0}code{font-family:ui-monospace,monospace}</style>" + body)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        target = unquote(self.path[1:])
        if not target:
            self.send_response(302)
            self.send_header("Location", "?" + args.initial_target)
            self.end_headers()
            return
        if not target.startswith("?"):
            self.send_error(404)
            return
        target = target[1:]
        contents = page(target, query(target)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(contents)

    def log_message(self, _format, *_arguments):
        pass


parser = argparse.ArgumentParser(prog="ninja -t browse")
parser.add_argument("--port", "-p", default=8000, type=int)
parser.add_argument("--hostname", "-a", default="localhost")
parser.add_argument("--no-browser", action="store_true")
parser.add_argument("--ninja-command", default="knight")
parser.add_argument("-f", default="build.ninja")
parser.add_argument("initial_target", default="all", nargs="?")
args = parser.parse_args()


class Server(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True


server = Server((args.hostname, args.port), Handler)
print("Web server running on %s:%d, ctl-C to abort..." % server.server_address)
if not args.no_browser:
    webbrowser.open_new("http://%s:%d" % server.server_address)
try:
    server.serve_forever()
except KeyboardInterrupt:
    print()
