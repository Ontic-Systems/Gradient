# Security docs

Internal security/threat-model documentation for Gradient. Most of this directory is reserved for sub-issues under Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model + sigstore-prep + sandbox + fuzz + DDC + reproducible builds).

| Doc | Issue | Status |
|---|---|---|
| [`effect-soundness.md`](effect-soundness.md) | [#363](https://github.com/Ontic-Systems/Gradient/issues/363) | sketch (informal proof, mechanization tracked as open question) |

Planned but not yet drafted (one row will be added per sub-issue PR):

- threat model overview ([#355](https://github.com/Ontic-Systems/Gradient/issues/355))
- comptime sandbox ([#356](https://github.com/Ontic-Systems/Gradient/issues/356))
- cargo-fuzz harness — lexer + parser ([#357](https://github.com/Ontic-Systems/Gradient/issues/357))
- cargo-fuzz harness — checker + IR builder ([#358](https://github.com/Ontic-Systems/Gradient/issues/358))
- LSP `@untrusted` default ([#359](https://github.com/Ontic-Systems/Gradient/issues/359))
- `@untrusted` source mode ([#360](https://github.com/Ontic-Systems/Gradient/issues/360))
- DDC bootstrap verification ([#361](https://github.com/Ontic-Systems/Gradient/issues/361))
- reproducible builds ([#362](https://github.com/Ontic-Systems/Gradient/issues/362))
- prompt-injection-resistant codegen guidelines ([#364](https://github.com/Ontic-Systems/Gradient/issues/364))
