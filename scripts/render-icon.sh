#!/bin/sh
# macOS: render the source SVG and package its PNGs into Buckets.icns.
set -eu
cd "$(dirname "$0")/.."
swift -module-cache-path target/swift-module-cache scripts/render-icon.swift
