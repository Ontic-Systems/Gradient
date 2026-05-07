# Security docs

Internal security/threat-model documentation for Gradient. Most of this directory is reserved for sub-issues under Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model + sigstore-prep + sandbox + fuzz + DDC + reproducible builds).

| Doc | Issue | Status |
|---|---|---|
| [`threat-model.md`](threat-model.md) | [#355](https://github.com/Ontic-Systems/Gradient/issues/355) | published — covers 10 surfaces + 2 tooling findings |
| [`effect-soundness.md`](effect-soundness.md) | [#363](https://github.com/Ontic-Systems/Gradient/issues/363) | sketch (informal proof, mechanization tracked as open question) |
| [`reproducible-builds.md`](reproducible-builds.md) | [#362](https://github.com/Ontic-Systems/Gradient/issues/362) | published — recipe + CI gate + known limitations (LLVM out of scope) |
| [`agent-codegen-guidelines.md`](agent-codegen-guidelines.md) | [#364](https://github.com/Ontic-Systems/Gradient/issues/364) | published — G1–G10 prompt-injection-resistant codegen rules (closes TF2) |
| [`ddc.md`](ddc.md) | [#361](https://github.com/Ontic-Systems/Gradient/issues/361) | published — DDC procedure + current obstacles + release-checklist hook (closes F6 deliverable) |
| [`release-checklist.md`](release-checklist.md) | [#361](https://github.com/Ontic-Systems/Gradient/issues/361) | published — three-tier release gate checklist |
| [`comptime-sandbox.md`](comptime-sandbox.md) | [#356](https://github.com/Ontic-Systems/Gradient/issues/356) | published — three-layer comptime sandbox (closes F2) |
| [`untrusted-source-mode.md`](untrusted-source-mode.md) | [#360](https://github.com/Ontic-Systems/Gradient/issues/360) | published — `@untrusted` source mode (closes F4 input surface) |
| [`fuzz-harness.md`](fuzz-harness.md) | [#357](https://github.com/Ontic-Systems/Gradient/issues/357), [#358](https://github.com/Ontic-Systems/Gradient/issues/358) | published — cargo-fuzz lexer + parser + checker + IR targets + nightly CI (closes F3) |

Planned but not yet drafted (one row will be added per sub-issue PR):

- LSP `@untrusted` default ([#359](https://github.com/Ontic-Systems/Gradient/issues/359))
