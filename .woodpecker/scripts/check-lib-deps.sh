#!/bin/sh
# Verify that no crate in libs/* declares a dependency on any crate in services/*.
# Service crate package names are those declared in each service's [package] name field.
set -eu

FAIL=0

for lib_toml in libs/*/Cargo.toml; do
  for svc in mcp-gateway mcp-api mcp-credential-injector; do
    if grep -q "\"${svc}\"" "${lib_toml}"; then
      echo "ERROR: ${lib_toml} declares a dependency on service crate '${svc}'"
      FAIL=1
    fi
  done
done

if [ "${FAIL}" -eq 1 ]; then
  echo "FAIL: library crates must not depend on service crates"
  exit 1
fi

echo "OK: no library crate depends on any service crate"
