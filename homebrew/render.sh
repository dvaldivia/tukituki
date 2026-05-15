#!/usr/bin/env bash
# Render homebrew/tukituki.rb.tmpl into a concrete formula by
# substituting __VERSION__ + the four SHA placeholders. The release
# workflow exports these as env vars; running locally for inspection:
#
#   VERSION=1.2.3 \
#   SHA_LINUX_X86_64=... SHA_LINUX_ARM64=... \
#   SHA_DARWIN_X86_64=... SHA_DARWIN_ARM64=... \
#     ./homebrew/render.sh > /tmp/tukituki.rb

set -euo pipefail

: "${VERSION:?VERSION is required}"
: "${SHA_LINUX_X86_64:?}"
: "${SHA_LINUX_ARM64:?}"
: "${SHA_DARWIN_X86_64:?}"
: "${SHA_DARWIN_ARM64:?}"

template_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sed \
  -e "s|__VERSION__|${VERSION}|g" \
  -e "s|__SHA_LINUX_X86_64__|${SHA_LINUX_X86_64}|g" \
  -e "s|__SHA_LINUX_ARM64__|${SHA_LINUX_ARM64}|g" \
  -e "s|__SHA_DARWIN_X86_64__|${SHA_DARWIN_X86_64}|g" \
  -e "s|__SHA_DARWIN_ARM64__|${SHA_DARWIN_ARM64}|g" \
  "$template_dir/tukituki.rb.tmpl"
