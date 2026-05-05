# ADR 0002: Arenas + capabilities (no lifetime annotations)

- Status: Accepted (locked 2026-05-02)
- Deciders: Gradient core (alignment session Q4/Q6)
- Epic: [#296](https://github.com/Ontic-Systems/Gradient/issues/296)
- Tracking issue: [#326](https://github.com/Ontic-Systems/Gradient/issues/326)
- Related ADRs: [ADR 0001](0001-effect-tier-foundation.md), [ADR 0005](0005-stdlib-split.md), [ADR 0006](0006-inference-modes.md)
- Related epics: effects ([#295](https://github.com/Ontic-Systems/Gradient/issues/295)), runtime ([#298](https://github.com/Ontic-Systems/Gradient/issues/298)), threat model ([#302](https://github.com/Ontic-Systems/Gradient/issues/302)), registry ([#303](https://github.com/Ontic-Systems/Gradient/issues/303))

## Context

[ADR 0001](0001-effect-tier-foundation.md) locks **what a function does** as effect rows. It does not answer **what tokens a function holds** to be allowed to do those things. That second question — authority, ownership, and the rules under which heap memory is freed — is the job of the memory + capability model defined here.

Three real constraints force the decision:

1. **The agent-emission failure mode for Rust-style borrowck.** Lifetime annotations (`<'a>(&'a mut T) -> &'a U`) are the single largest source of LLM-hostile dialogue in mainstream systems languages. They are inferable in many cases, but not in any way an agent can predict before invoking the checker, so the agent learns to over-annotate, which produces noisy diagnostics, which feeds back into more over-annotation. Gradient's first-class user is an LLM; we cannot ship that loop.
2. **The bare-metal floor.** Gradient targets `no_std` firmware and kernels (per the [vision roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02)). On that tier there is no global allocator; everything that wants memory either lives on the stack, in `@static` storage, or in an explicit allocator instance. Reference counting alone (Gradient's current heap discipline, see [`gradient-cow-memory-management`](../../#) skill) cannot describe a hardware page table or a per-request request arena.
3. **The audit trail for `extern fn`.** F5 of the 2026-05-02 adversarial review flagged that `extern fn` is currently ungated — any module can call into C without surfacing it on the call graph. Capabilities give us the discipline to require an audit token (`Unsafe`) at the call site of every C boundary, and effect rows ([ADR 0001](0001-effect-tier-foundation.md)'s `!{FFI(C)}`) give us the propagation up the call chain.

We need a single mechanism that:

- Replaces lifetime annotations entirely. No `<'a>`, no `for<'a>`, no `'static`.
- Lets a function declare *what authority it needs* and *which arena it allocates from* without a per-feature special form.
- Composes through call chains — the checker can tell an agent "this caller needs to hold `Unsafe` because line 42 of the callee requires it" without dialogue.
- Is machine-readable on signatures (alongside effect rows) and machine-checkable (capability typestate).
- Forbids forging or smuggling capability tokens at compile time.
- Lands incrementally on top of the effect-tier foundation already accepted.

## Decision

**Memory ownership and authority are expressed as two layered things on a function signature: an effect row (already from [ADR 0001](0001-effect-tier-foundation.md)) and a capability set.** A capability is a typestate token threaded through the program by ordinary value-passing rules; it is not a phantom annotation, it is not inferred from lifetime relationships, and it carries no per-call dialogue with the checker. The capability typestate engine + the arena allocator runtime crate are the two new substrates this ADR locks in.

Concretely:

- **No lifetime annotations anywhere.** No `'a`, no implicit `'static` rebinding, no HRTB. The grammar reserves no syntax for them.
- **All heap-shaped storage is owned by an explicit allocator value** — the global allocator (gated by `!{Heap}`), an arena, an arena pool, or a hardware-fixed `@static` region. Refcount + COW (Gradient's current discipline) becomes one of several allocation strategies, not the only one.
- **Authority to do dangerous things is carried by capability tokens** — first-class values that the checker tracks linearly through the program. `Unsafe`, `Filesystem`, `Network`, `Spawn`, `RawPointer`, and `Hardware` are the launch set; a registry-driven manifest (Epic [#303](https://github.com/Ontic-Systems/Gradient/issues/303), [ADR 0007](#) when it lands) extends the set per-package.
- **C ABI is the FFI baseline.** `extern fn` declarations require the caller to hold `Unsafe` and contribute `!{FFI(C)}` to the effect row. Other ABIs are explicit additional capabilities (`UnsafeWasm`, `UnsafeSysv`, etc.); the default path stays C ABI.

### Arena model (Epic E3, sub-issue [#320](https://github.com/Ontic-Systems/Gradient/issues/320))

An arena is a value of type `Arena[T]` (or the type-erased `Arena.Any`) whose lifetime is the lifetime of the binding. It exposes:

```gradient
let arena = Arena.new()                  // !{Heap}
let buf   = arena.alloc(BigStruct.zero) // returns Arena.Ref[BigStruct]
arena.reset()                            // bump pointer back; live refs invalidated
arena.drop()                             // implicit at end-of-scope
```

| Arena kind | Backing storage | Effects on `alloc` | Sub-issue |
|---|---|---|---|
| `Arena` (bump) | global allocator chunks | `!{Heap}` | [#320](https://github.com/Ontic-Systems/Gradient/issues/320) |
| `Arena.Stack` | caller's stack frame | `!{Stack}` | [#320](https://github.com/Ontic-Systems/Gradient/issues/320) |
| `Arena.Static[size]` | `@static` byte buffer | `!{Static}` | [#320](https://github.com/Ontic-Systems/Gradient/issues/320) |
| `Arena.Pool` | recycled chunks across resets | `!{Heap}` | follow-on under E3 |

The arena's references (`Arena.Ref[T]`) are values: they implement `@move` semantics where applicable, but they are **not** lifetime-annotated. The checker proves liveness by tracking the arena's typestate — see below.

**No raw `&T` references in the surface language.** All shared access goes through arena references, refcount handles, capability-bound resources, or copy-by-value. Pointer types (`@ptr T`, `@ptr_mut T`) exist for FFI and require the `RawPointer` capability to construct.

### Capability typestate (sub-issue [#321](https://github.com/Ontic-Systems/Gradient/issues/321))

A capability is a value of a sealed type. The checker tracks each capability through three typestate axes:

1. **Held / not held.** A function may declare `cap unsafe: Unsafe` in its signature; the caller must pass an `Unsafe` token in. Capabilities cannot be forged from `()` or default-constructed.
2. **Live / consumed.** Capabilities are linear by default — once consumed (passed to a function that takes ownership), the binding is moved out and cannot be reused. `cap.clone()` is itself an authority op gated on a capability's `Cloneable` marker; default capabilities are NOT cloneable.
3. **Scoped to an arena (when applicable).** Capabilities that mediate access to an arena's contents (`Arena.WriteCap`, `Arena.ReadCap`) are typestate-tied to that arena's identity; the checker rejects use across the arena's `reset()` or `drop()`.

Surface syntax (locked):

```gradient
fn read_config(fs: Filesystem, path: String) -> String !{FS, Throws(IOError)} {
    fs.read_to_string(path)?
}

fn unsafe_pack(unsafe: Unsafe, raw: @ptr Byte, len: Int) -> Bytes !{FFI(C)} {
    Bytes.from_raw(unsafe, raw, len)
}

@app  // see ADR 0006 — @app default
fn main() -> Int {
    let fs = capability::root_filesystem()  // capability provider; root-only
    let cfg = read_config(fs, "config.toml")
    print(cfg)
    0
}
```

`capability::root_filesystem()` and friends live in the `core::cap` module and are themselves gated on the program's threat-model annotation (`@trusted` only, see Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302)).

### Capability launch set (sub-issues [#321](https://github.com/Ontic-Systems/Gradient/issues/321), [#322](https://github.com/Ontic-Systems/Gradient/issues/322))

| Capability | Gates | Paired effect | Provider | Cloneable |
|---|---|---|---|---|
| `Unsafe` | `extern fn`, raw pointer construction, ABI escape hatches | `!{FFI(_)}` or `!{Mut}` depending on op | `core::cap::root_unsafe()` (`@trusted` only) | no |
| `Filesystem` | stdlib FS ops, `gradient`-shipped IO builtins that touch disk | `!{FS}` | `core::cap::root_filesystem()` (`@trusted`) | no |
| `Network` | sockets, DNS, HTTP clients | `!{Net}` | `core::cap::root_network()` | no |
| `Spawn` | actor spawn, OS process spawn | `!{Async}` (actor) or `!{IO}` (process) | `core::cap::root_spawn()` | no |
| `RawPointer` | `@ptr T`, `@ptr_mut T`, pointer arithmetic | `!{Mut}` | derived from `Unsafe` only | no |
| `Hardware` | `@volatile`, MMIO, `!{Volatile}` | `!{Volatile}` + `!{Static}` | `core::cap::root_hardware()` (firmware-only) | no |

This is the launch set. Additional capabilities are package-defined and surface through the registry manifest ([#303](https://github.com/Ontic-Systems/Gradient/issues/303), ADR 0007).

### Forbidden combinations

The checker rejects:

- Constructing `@ptr T` or `@ptr_mut T` without holding `RawPointer`.
- Calling an `extern fn` without holding `Unsafe`.
- Using an `Arena.Ref[T]` after the arena it came from is dropped or reset.
- Dropping a linear capability before it has been consumed (warning by default; error under `@panic(none)`).
- Cloning a non-`Cloneable` capability.
- Forging a capability via `transmute`-style operations (`transmute` itself requires `Unsafe` and is documented to violate type safety).

### `Unsafe` capability gate on `extern fn` (sub-issue [#322](https://github.com/Ontic-Systems/Gradient/issues/322))

Every `extern fn` declaration is rewritten by the checker to require `cap unsafe: Unsafe` at the call site. The effect row gains `!{FFI(C)}` (or the appropriate ABI variant). This closes adversarial finding F5 from the 2026-05-02 review.

Migration is staged so existing self-hosted modules are not invalidated overnight:

1. The compiler accepts a `--unsafe-extern-implicit` flag during the dogfood window that injects an implicit `Unsafe` for every call to a kernel `bootstrap_*` extern. The flag is on by default, off in CI release builds.
2. Sub-issue [#322](https://github.com/Ontic-Systems/Gradient/issues/322) annotates every `bootstrap_*` extern + every kernel-boundary call site in `compiler/*.gr`.
3. Sub-issue [#325](https://github.com/Ontic-Systems/Gradient/issues/325) flips one self-hosted module to capability-passing as a dogfood proof.
4. The flag flips off by default once the trust gate is green with capabilities enabled.

### `@repr(C)` struct layout (sub-issue [#323](https://github.com/Ontic-Systems/Gradient/issues/323))

`@repr(C)` is a struct attribute that pins layout to the platform C ABI: field order = declaration order, alignment per platform rules, no implicit padding insertion beyond what C requires, no auto-deriving niche optimizations. It is paired with `@repr(packed)`, `@repr(transparent)`, and `@repr(align(N))` as additional opt-ins. Default layout remains the Gradient native one (free to reorder for niche-packing).

### `gradient bindgen` MVP (sub-issue [#324](https://github.com/Ontic-Systems/Gradient/issues/324))

`gradient bindgen <header.h>` emits a `.gr` file with `extern fn` declarations matching the C header's exported symbols. MVP scope:

- C99 prototypes including primitive types, pointers, and `struct` types tagged `@repr(C)`.
- One `Unsafe` capability gate inserted on the module, applied to every emitted extern.
- Effect rows default to `!{FFI(C), IO, Net, FS, Mut, Time}` (the same conservative default `EXTERN_DEFAULT_EFFECTS` produces today). Refinement is left to the consumer.
- Out of scope for MVP: macros, inline functions, C++ overloads, generics, conditional includes — these may be lifted to follow-on issues.

## Consequences

### Positive

- **No lifetime annotations.** Agents emit Gradient code without the borrow-checker dialogue loop. The capability typestate engine produces structurally simpler diagnostics — "missing `Unsafe` at line N" vs "borrow checker detected conflicting borrows of foo at lines K, L, M".
- **Bare metal becomes definable.** `Arena.Static[size]` + `@volatile` + `Hardware` capability + `@panic(abort)` is sufficient to emit a kernel module without invoking the heap.
- **`extern fn` audit trail.** Every C boundary is now visible on the call graph through `Unsafe` propagation. The threat model (Epic E9, [#302](https://github.com/Ontic-Systems/Gradient/issues/302)) plus the registry (Epic E10, [#303](https://github.com/Ontic-Systems/Gradient/issues/303)) can use this directly: a package manifest declares which capabilities its public API touches, and sigstore verification gates which packages are allowed to acquire root capabilities.
- **Composability with effects.** Effects answer "what does this fn do"; capabilities answer "which tokens does it need to do it". Both surface on the signature; both propagate; both inform linker DCE (Epic E5, [#298](https://github.com/Ontic-Systems/Gradient/issues/298)) and inference defaults (Epic E8, [ADR 0006](0006-inference-modes.md)).
- **No global ambient authority.** Outside `@trusted` modules, capabilities cannot be conjured — every authority enters the program through `main`'s entrypoint or a `@trusted` capability provider. This locks the agent threat model: an `@untrusted` plugin cannot acquire `Filesystem` without the host explicitly handing one over.

### Negative

- **More noise on signatures** for code that touches multiple capabilities or arenas. Mitigated by inference (Epic E8): in `@app` mode, capabilities for the standard set (`Filesystem`, `Network`) are auto-threaded through unannotated leaf functions when the call site provides them; `@system` mode requires explicit declaration.
- **`extern fn` migration is intrusive.** Every existing `bootstrap_*` extern needs an annotation pass; every self-hosted call site needs a capability passed in. Mitigation is the staged dogfood window above.
- **Arenas are an unfamiliar idiom for app-tier developers.** Most app code will never touch them — refcount + global allocator (`!{Heap}`) covers the same surface. Arenas are an opt-in for systems work and request-scoped patterns. We accept the documentation cost.
- **No raw `&T` is genuinely a tradeoff.** Some idioms (in-place container mutation behind a method that takes `&mut self`) require either refcount handles or arena references in Gradient. The cost in surface familiarity is what buys the agent-emission win and the elimination of lifetime annotations.

### Neutral / deferred

- **Region polymorphism / arena-generic functions.** Locked out of the launch set. A function that allocates into "whichever arena the caller has" requires either capability-based arena threading (already supported) or explicit type-level arena identity (deferred to a future ADR if the need is real).
- **Linear types beyond capabilities.** The capability typestate engine is restricted to capability values + arena references. We do not (yet) generalize linearity to user-defined types. Affine / linear user types may land later if the demand surfaces.
- **GPU memory.** Deferred post-1.0 per Q7. A future capability `GpuDevice` and effect `!{GPU(_)}` are anticipated but not on the launch set.
- **Capability revocation.** Capabilities cannot be revoked once handed to a callee that owns them. Revocable capabilities are a deferred feature; the workaround for now is to scope the consumer.

## Implementation order

Sub-issues land in this order so each adds a checker rule + at least one dogfood:

1. [#320](https://github.com/Ontic-Systems/Gradient/issues/320) `Arena` runtime crate + `Arena`/`Arena.Stack`/`Arena.Static` constructors. Needed first because the typestate engine references `Arena.Ref[T]` typestate.
2. [#321](https://github.com/Ontic-Systems/Gradient/issues/321) Capability typestate engine in the checker. Tracks held/consumed/cloneable per binding; adds the `cap name: Cap` parameter syntax. Includes the launch-set capability declarations in `core::cap`.
3. [#322](https://github.com/Ontic-Systems/Gradient/issues/322) `Unsafe` capability gate on `extern fn` + `!{FFI(C)}` effect. Closes F5. Includes `--unsafe-extern-implicit` flag and migration plan.
4. [#323](https://github.com/Ontic-Systems/Gradient/issues/323) `@repr(C)` struct layout. Required by `gradient bindgen` for type-safe emission.
5. [#324](https://github.com/Ontic-Systems/Gradient/issues/324) `gradient bindgen` MVP. Consumes [#323](https://github.com/Ontic-Systems/Gradient/issues/323) and [#322](https://github.com/Ontic-Systems/Gradient/issues/322).
6. [#325](https://github.com/Ontic-Systems/Gradient/issues/325) Migrate one `compiler/*.gr` module to capability-passing as a dogfood proof. Recommended target: `compiler/lsp.gr`'s extern surface — it has a narrow, well-known set of `bootstrap_lsp_*` calls and the trust gate already covers it.

Each sub-issue includes:

- Parser / surface syntax extension if required (most reuse `cap`, `Arena`, and `@repr` grammar).
- Checker rule + diagnostic with the canonical "missing capability" / "arena reference outlives arena" / "linear capability dropped" errors.
- Codegen path: arenas + capabilities are runtime-erased after typecheck; `@repr(C)` lowers to a layout pin; `bindgen` is a host-side tool.
- At least one stdlib/runtime annotation update.
- At least one self-hosted module dogfood under [#382](https://github.com/Ontic-Systems/Gradient/issues/382).

## Comparison

### vs Rust borrowck

| Dimension | Rust | Gradient (this ADR) |
|---|---|---|
| Lifetime annotations | `<'a>(&'a T) -> &'a U` | none — capability + arena typestate |
| Default ownership | move + borrow | move + refcount + arena reference |
| FFI gate | `unsafe` block | `Unsafe` capability + `!{FFI(C)}` |
| Bare-metal storage | `'static` lifetime | `Arena.Static[size]` + `@static` |
| Diagnostic shape | "borrow X at line K conflicts with borrow Y at line L" | "missing capability `Unsafe` at line N" / "arena reference outlives arena drop at line N" |
| Agent emission | dialogue-heavy | structurally simple |

The trade is real: Rust's borrowck buys zero-overhead aliasing analysis on shared mutable references, which Gradient does not match. Gradient's response is "don't expose raw shared mutable references in the surface language at all"; the gap is closed by refcount + arenas, both of which are runtime-checked or layout-checked rather than lifetime-checked.

### vs refcount-only

Refcount + COW (Gradient's current heap discipline; see the [`gradient-cow-memory-management`](../../#) skill) handles everything that lives at app tier with a global allocator. It does not handle:

- Bare-metal: there is no global allocator. Refcount + COW require one.
- Request-scoped allocation: refcount churn through a per-request graph is allocator-dominated work; an arena that resets at end-of-request is asymptotically faster.
- Hardware MMIO: refcount cannot describe a fixed-address page. `@volatile` + `Hardware` capability + `@static` arena handles it.

We keep refcount + COW as the default at app tier (it is what `@app` mode infers when `!{Heap}` is permitted) and add arenas as the systems-tier alternative. The two do not conflict; arena references are themselves a refcount-free subkind of value.

## Related

- [ADR 0001](0001-effect-tier-foundation.md) — effect tier; `!{Heap}`, `!{Stack}`, `!{Static}`, `!{FFI(C)}`, `!{Mut}` are the effect rows this ADR's capabilities pair with.
- [ADR 0005](0005-stdlib-split.md) — `core::cap` lives in `core` (no allocator dependency); `core::arena` lives in `alloc`. The split lines up cleanly.
- [ADR 0006](0006-inference-modes.md) — `@app`/`@system` modes choose how aggressively capabilities are inferred vs explicit.
- ADR 0007 (planned, Epic E10) — registry trust model; capability-scoped manifests reference the launch set defined here.
- Epic E2 [#295](https://github.com/Ontic-Systems/Gradient/issues/295) — effects.
- Epic E3 [#296](https://github.com/Ontic-Systems/Gradient/issues/296) — this ADR's parent.
- Epic E5 [#298](https://github.com/Ontic-Systems/Gradient/issues/298) — modular runtime; the `core`/`alloc`/`std` split's effect-driven DCE applies to capability-bound calls too.
- Epic E9 [#302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model; `@trusted`/`@untrusted` decide which modules can call capability providers.
- Epic E10 [#303](https://github.com/Ontic-Systems/Gradient/issues/303) — registry; manifests declare which capabilities a package's public API requires.
- Epic E12 [#116](https://github.com/Ontic-Systems/Gradient/issues/116) — self-hosting; sub-issue [#325](https://github.com/Ontic-Systems/Gradient/issues/325) dogfoods one module onto capability-passing.
- Roadmap: [`docs/roadmap.md` § Vision Roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02).

## Notes

The Q-numbered decisions referenced inline (Q4 for arenas + capabilities, Q6 for C ABI + `Unsafe` gate) are the alignment-session questions that locked each piece. The session log is internal-only; this ADR is the canonical public record.

Adversarial finding F5 from the 2026-05-02 review ("`extern fn` ungated") is closed by [#322](https://github.com/Ontic-Systems/Gradient/issues/322) under this ADR.
