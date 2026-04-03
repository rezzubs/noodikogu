# Overview

- This is a catalogue for organizing musical scores with metadata. 
- The core design is covered in `design.md`.

# Coding

- Always check code with `cargo clippy` instead of `cargo check`.
- Tests are ran with `cargo test`
- Always write doc comments for your types and functions
- Do not inlcude private struct fields in doc comments
- Always derive common traits like Debug, Clone, Copy, PartialEq, Eq, Hash when appropriate.
- impl blocks should come right after the type (no other types in the middle). Trait impl blocks should follow regular impl blocks.
