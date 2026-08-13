#!/usr/bin/env python3
# Copyright 2001 Google Inc. All Rights Reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""Simple web server for browsing dependency graph data."""

import argparse
from collections import namedtuple
from html import escape
import http.server as httpserver
import os
import socket
import socketserver
import subprocess
import sys
from typing import Any, Tuple
from urllib.parse import quote, unquote
import webbrowser


Node = namedtuple("Node", ["inputs", "rule", "target", "outputs"])


def match_strip(line: str, prefix: str) -> Tuple[bool, str]:
    if not line.startswith(prefix):
        return (False, line)
    return (True, line[len(prefix):])


def html_escape(text: str) -> str:
    return escape(text, quote=True)


def parse(text: str) -> Node:
    lines = iter(text.split("\n"))
    target = None
    rule = None
    inputs = []
    outputs = []

    try:
        target = next(lines)[:-1]
        line = next(lines)
        (match, rule) = match_strip(line, "  input: ")
        if match:
            (match, line) = match_strip(next(lines), "    ")
            while match:
                input_type = ""
                (match, line) = match_strip(line, "| ")
                if match:
                    input_type = "implicit"
                (match, line) = match_strip(line, "|| ")
                if match:
                    input_type = "order-only"
                inputs.append((line, input_type))
                (match, line) = match_strip(next(lines), "    ")

        match, _ = match_strip(line, "  outputs:")
        if match:
            (match, line) = match_strip(next(lines), "    ")
            while match:
                outputs.append(line)
                (match, line) = match_strip(next(lines), "    ")
    except StopIteration:
        pass

    return Node(inputs, rule, target, outputs)


def create_page(body: str) -> str:
    return """<!DOCTYPE html>
<style>
body {
    font-family: sans;
    font-size: 0.8em;
    margin: 4ex;
}
h1 {
    font-weight: normal;
    font-size: 140%;
    text-align: center;
    margin: 0;
}
h2 {
    font-weight: normal;
    font-size: 120%;
}
tt {
    font-family: WebKitHack, monospace;
    white-space: nowrap;
}
.filelist {
  -webkit-columns: auto 2;
}
</style>
""" + body


def generate_html(node: Node) -> str:
    document = ["<h1><tt>%s</tt></h1>" % html_escape(node.target)]
    if node.inputs:
        document.append(
            "<h2>target is built using rule <tt>%s</tt> of</h2>"
            % html_escape(node.rule)
        )
        document.append("<div class=filelist>")
        for input_path, input_type in sorted(node.inputs):
            extra = ""
            if input_type:
                extra = " (%s)" % html_escape(input_type)
            document.append(
                '<tt><a href="?%s">%s</a>%s</tt><br>'
                % (quote(input_path), html_escape(input_path), extra)
            )
        document.append("</div>")

    if node.outputs:
        document.append("<h2>dependent edges build:</h2>")
        document.append("<div class=filelist>")
        for output in sorted(node.outputs):
            document.append(
                '<tt><a href="?%s">%s</a></tt><br>'
                % (quote(output), html_escape(output))
            )
        document.append("</div>")
    return "\n".join(document)


def ninja_dump(target: str) -> Tuple[str, str, int]:
    command = [args.ninja_command, "-f", args.f, "-t", "query", target]
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
    )
    return process.communicate() + (process.returncode,)


class RequestHandler(httpserver.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        assert self.path[0] == "/"
        target = unquote(self.path[1:])
        if target == "":
            self.send_response(302)
            self.send_header("Location", "?" + args.initial_target)
            self.end_headers()
            return
        if not target.startswith("?"):
            self.send_response(404)
            self.end_headers()
            return
        target = target[1:]

        ninja_output, ninja_error, exit_code = ninja_dump(target)
        if exit_code == 0:
            page_body = generate_html(parse(ninja_output.strip()))
        else:
            page_body = "<h1><tt>%s</tt></h1>" % html_escape(ninja_error)

        self.send_response(200)
        self.end_headers()
        self.wfile.write(create_page(page_body).encode("utf-8"))

    def log_message(self, format: str, *arguments: Any) -> None:
        pass


parser = argparse.ArgumentParser(prog="ninja -t browse")
parser.add_argument(
    "--port", "-p", default=8000, type=int,
    help="Port number to use (default %(default)d)",
)
parser.add_argument(
    "--hostname", "-a", default="localhost", type=str,
    help="Hostname to bind to (default %(default)s)",
)
parser.add_argument(
    "--no-browser", action="store_true",
    help="Do not open a webbrowser on startup.",
)
parser.add_argument(
    "--ninja-command", default="ninja",
    help="Path to ninja binary (default %(default)s)",
)
parser.add_argument(
    "-f", default="build.ninja",
    help="Path to build.ninja file (default %(default)s)",
)
parser.add_argument(
    "initial_target", default="all", nargs="?",
    help="Initial target to show (default %(default)s)",
)


class HTTPServer(socketserver.ThreadingMixIn, httpserver.HTTPServer):
    daemon_threads = True


args = parser.parse_args()
port = args.port
hostname = args.hostname
httpd = HTTPServer((hostname, port), RequestHandler)
try:
    if hostname == "":
        hostname = socket.gethostname()
    print("Web server running on %s:%d, ctl-C to abort..." % (hostname, port))
    print("Web server pid %d" % os.getpid(), file=sys.stderr)
    if not args.no_browser:
        webbrowser.open_new("http://%s:%s" % (hostname, port))
    httpd.serve_forever()
except KeyboardInterrupt:
    print()
