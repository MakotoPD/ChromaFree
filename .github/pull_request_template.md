## Summary

Describe what this pull request changes and why.

Closes #

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Performance improvement
- [ ] Translation
- [ ] Documentation
- [ ] Build, installer or CI

## Testing

Describe how you tested the change: automated tests, apps used with the virtual camera, GPU and camera.

## Resource usage

For changes in capture, processing or the virtual camera: CPU, memory and latency before and after, measured at
1280x720 and 30 fps if possible. Write "not affected" otherwise.

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] The C++ virtual camera source builds without warnings (if changed)
- [ ] User-visible texts use `@tr()` in Slint or `i18n::tr` in Rust, with the Polish translation updated
- [ ] README or release notes are updated where needed
