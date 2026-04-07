## Summary
Teaches the parser to accept indented typed expressions whose value is provided on the next indented line, fixing a self-hosting blocker for enum-constructor forms like `T:\n    V(n: 1)`.

## Changes
- add a shared helper for parsing typed-expression values after a colon, including optional newline/indent handling
- route both generic typed expressions and simple named-type expressions through that helper
- add a parser regression test covering the indented typed-constructor form

## Testing
- `cargo test -p gradient-compiler parse_indented_typed_constructor_expr -- --nocapture`
- `cargo test`
- manual CLI repro with `./target/debug/gradient-compiler /tmp/grad_typed_expr.gr /tmp/out.o --parse-only`

## Related
Fixes #43
