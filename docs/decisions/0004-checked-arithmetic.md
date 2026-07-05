# Checked arithmetic in release builds

**Status**: Accepted

## Decision

We enabled overflow checks in release builds.

```toml
[profile.release]
overflow-checks = true
```

## Why

Rust enables overflow checks in debug builds but not in release builds. We want
to avoid invalid state in production no matter what. If we hit a path where the
checks have a measurable performance impact it's still possible to use
`wrapping_*` arithmetic operations manually.
