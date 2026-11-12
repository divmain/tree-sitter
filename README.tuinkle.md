# tuinkle fork of tree-sitter

This branch is upstream tree-sitter `v0.27.2` plus the narrow patches
below. Each patch is carried in its own commits (the patch, its fixes, and
its tests), and every hunk is fenced in the source so it can be re-ported
or dropped on its own:

- the Wasm store resource-policy extension is marked `EXTENSION:`;
- the other patches are fenced `tuinkle: <NAME>` / `tuinkle: end <NAME>`.

`rg -n 'tuinkle: |EXTENSION:'` lists every patched site. Nothing outside the
fences differs from upstream, except where a section below says so.

## Wasm store resource-policy extension

Files: `lib/include/tree_sitter/api.h`, `lib/src/wasm_store.c`,
`lib/binding_rust/bindings.rs`, `lib/binding_rust/wasm_language.rs`
(markers: `EXTENSION:`).

API added (all behind the `wasm` feature):

- C: `TSWasmStoreOptions { fuel, memory_limit_bytes }`,
  `ts_wasm_store_new_with_options(engine, options, error)`,
  `ts_wasm_store_fuel_remaining(store, &fuel)`, and the error kind
  `TSWasmErrorKindResource`.
- Rust: `WasmStoreOptions`, `WasmStore::new_with_options`,
  `WasmStore::fuel_remaining`, and `WasmErrorKind::Resource`.

Semantics:

- `fuel` is installed on the store before any guest code runs. The engine
  must have fuel consumption enabled, or construction fails with the
  resource error kind.
- `memory_limit_bytes` is installed as the store's wasmtime resource
  limiter before the store's memory is created, and the memory's declared
  maximum is lowered to agree. A limit above the integration's 128 MiB
  maximum, below the Wasm stdlib's initial memory, or too small for the
  lexer and serialization buffer fails construction. No value is silently
  clamped.
- `ts_wasm_store_new` / `WasmStore::new` are unchanged: they call the same
  constructor with no options. On a fuel-enabled engine they fail, because
  a new wasmtime store holds no fuel; use `new_with_options` there.
- Construction runs guest code (the Wasm stdlib's initializer), so it
  spends some of the installed fuel. A budget too small for that fails with
  `WasmErrorKind::Instantiate` (a fuel trap), not `Resource`. Fuel that
  runs out during a parse makes the parse return `None`; the store's fuel
  stays exhausted, so a caller starts a fresh store for the next attempt.
- `ts_wasm_store_reset_heap` skips its guest call when fuel is enabled and
  exhausted, instead of tripping upstream's `ts_assert(!trap)`.
- The constructor's error path no longer double-frees `import_types`,
  `export_types`, `stdlib_module`, or `lexer_funcs` when a failure happens
  after they were released (latent upstream bugs that the new failure
  points can reach), and a denied lexer-memory grow fails construction
  instead of being ignored.

Why: tree-sitter constructs the `wasmtime` store itself, inside
`ts_wasm_store_new`, and never exposes it. A caller holds only the engine,
so there is no other way to give the store a fuel budget or a memory
ceiling. Running untrusted Wasm grammars (and their external scanners)
needs both.

Tests: none in this fork yet; the upstream `wasm_language_test` suite
covers the unchanged `WasmStore::new` path.

Exit condition: drop the patch once upstream exposes store configuration
(fuel and a memory limiter, or the store itself) at construction time.

## HL-PARSE-BUDGET

File: `crates/highlight/src/highlight.rs` (fence: `tuinkle: HL-PARSE-BUDGET`).
Tests: `crates/cli/src/tests/highlight_test.rs`, the fenced
`test_highlighting_with_*parse_budget*` tests.

API added: `Highlighter::highlight_with_parse_budget(config, source,
encoding, cancellation_flag, parse_budget: Option<Duration>,
injection_callback)`. `Highlighter::highlight` delegates to it with `None`
and behaves exactly as upstream.

Every parse the highlighter drives gets its own deadline, measured from the
start of that parse: the root layer, each combined injection (parsed
eagerly inside the call), and each injected layer constructed lazily during
iteration. The deadline is checked in the parse progress callback beside
the cancellation flag. An expired parse halts and surfaces as
`Error::Cancelled`, from the call itself or from the iterator, the same as
a set cancellation flag. The budget is a per-call argument, so no budget
state stays on the highlighter's parser; a halted highlighter is reusable.

The parser invokes the progress callback every 100 parse operations, so a
parse shorter than that never observes its deadline. The budget bounds
parsing only, not query execution.

Why: `Parser::set_timeout_micros` was removed in 0.26 in favour of the
parse progress callback, and `Highlighter::highlight` installs its own
progress callback that checks only the cancellation flag. The highlighter
owns its parser, so a caller had no way to bound the parses it runs. A
cancellation flag alone needs a separate timer thread and cannot give each
injected parse its own budget.

Exit condition: drop the patch once upstream's highlighter accepts a
caller-supplied progress callback or parse options for every parse it
drives (root, combined, and injected layers).

## REGISTRY-LANGUAGE

File: root `Cargo.toml` (fence: `tuinkle: REGISTRY-LANGUAGE`).

The workspace dependency `tree-sitter-language` is taken from crates.io
(`version = "0.1.9"`) instead of by path, and a workspace-root
`[patch.crates-io]` entry points it back at `./crates/language`, so this
workspace still builds the member. The crates.io 0.1.9 release is
byte-identical to `crates/language`.

Why: `tree-sitter-language` declares `links = "tree-sitter-language"`, and
cargo allows only one package with a given `links` value per dependency
graph. Every grammar crate depends on the crates.io release. Upstream's
`tree-sitter` takes it by path, so a project that depends on this fork by
git would get the git copy beside the registry copy that its grammar
crates pull in: two packages with the same `links` key, and resolution
fails. `[patch.crates-io]` is honoured only in the root manifest of a
build, so in a git dependent's build the patch is inert and the fork's
`tree-sitter` resolves the same registry package as the grammars.

Exit condition: drop the patch once upstream's workspace takes
`tree-sitter-language` from the registry (so git dependents already
unify with grammar crates), or once no dependent consumes this fork by git.
If `crates/language` changes in a newer base, the registry version must be
bumped to a release that is byte-identical to it.

## SEC-QUERY-DEPTH

File: `lib/src/query.c` (fence: `tuinkle: SEC-QUERY-DEPTH`).
Test: `crates/cli/src/tests/query_test.rs`,
`test_query_captures_past_the_16_bit_tree_depth` (fenced).

`QueryState::start_depth` is widened from `uint16_t` to `uint32_t`, the
type of the cursor's own `depth`.

Each in-progress match records the tree depth where its pattern started.
`ts_query_cursor__add_state` stores `self->depth - step->depth` into that
field. Upstream's `uint16_t` wraps once the tree is deeper than 65535, and
every later comparison against the `uint32_t` cursor depth then misfires:

- the leave-node pruning (`start_depth > depth`,
  `start_depth + step->depth > depth`) never removes the wrapped states;
- `ts_query_cursor__should_descend` keeps descending for them.

The wrapped states pile up and every cursor step rescans them, so query
iteration goes quadratic. Single `QueryCursor::captures` steps run for
seconds, inside one `HighlightIter::next` call that never reaches the
highlighter's cancellation check. The cursor also returns wrong output:
every capture past depth 65535 is dropped.

The cliff is the tree depth alone, whatever the source shape. Nested Rust
blocks add two levels per `{`, so the cliff is 32 768 nested blocks
(64 KiB). Nested parentheses add one level per `(`, so it is 65 536 of
them. A macro token tree holding nested blocks stays shallow in the main
layer (one level per brace), but the injected Rust layer re-parses the
same bytes two levels per brace and hits the cliff on its own. No
request-side bound can reach this: the injection callback sees only a
language name, and the highlighter owns its query cursor.

The patch has three fenced hunks:

1. The field width. On this base `QueryState` stays 24 bytes (the wider
   field fills existing padding; measured with clang on arm64). The layout
   is internal to `query.c`, and the public API and ABI are unchanged.
2. `ts_query_cursor__add_state` computes the start depth explicitly where
   `self->depth - step->depth` underflows. A pattern whose root is
   optional (`(x)*`, `(x)?`) has a pattern-map entry at its DONE step,
   whose depth is the `UINT16_MAX` marker; a wildcard-root pattern starts
   at a depth-1 step, which underflows at the traversal root. Upstream's
   16-bit field turned these into start depths that never satisfy
   `start_depth + step->depth == depth`. With the field widened alone, the
   sum wraps back to `depth` exactly, so the cursor matched the DONE step
   and stepped past the end of the pattern at every depth: upstream's
   `test_consecutive_zero_or_modifiers` hit a `ts_assert` (and release
   builds read past the query's steps). The hunk stores upstream's values:
   `depth + 1` for the DONE step, and the 16-bit wrapped value for the
   root-level underflow. Results inside the 16-bit range are identical to
   upstream's.
3. The cursor's node-matching loop skips states at their DONE step
   explicitly. A finished match deferred in case a longer one follows
   (for example `(source_file (function_item)* @f)`, deferred until the
   cursor leaves the root) stays in progress while the cursor descends.
   Its `start_depth + 65535` equals the cursor depth 65535 levels below its
   start, and the cursor then advances it past the end of its pattern.
   This one is an upstream bug, independent of the field width: v0.27.2
   asserts (debug) or reads out of bounds (release) on such a query over a
   tree deeper than 65535.

Tests (`crates/cli/src/tests/query_test.rs`, fenced; count/set assertions
only, no wall time):

- `test_query_captures_past_the_16_bit_tree_depth` parses 40 000 nested
  Rust blocks with the `rust` fixture grammar (tree depth ~80 000, which
  the test asserts exceeds 65535) and runs a raw `QueryCursor` over the
  fixture's `highlights.scm`. It asserts the full capture count
  (`2 * 40 000 + 4`) and that the innermost brace is captured. With a
  `uint16_t` field it fails by count (65 538 captures instead of 80 004).
- `test_query_finished_state_past_the_16_bit_tree_depth` runs
  `(source_file (function_item)* @function)` over the same deep tree and
  asserts the single match. Without hunk 3 (and on upstream v0.27.2) it
  aborts on the `ts_assert`.
- Hunk 2 is covered by upstream's own `test_consecutive_zero_or_modifiers`.

### Measurements

TODO(re-measure): the table below was measured on tree-sitter 0.25.10, not
on this v0.27.2 base. It is pending re-measurement here; replace it (and
this line) when that is done.

Measured on 0.25.10 with the downstream tuinkle harness
(`highlight_deep_tree_cost`): one-shot `highlight_by_name` requests,
median of 3, release, Linux `x86_64`, AMD Ryzen AI MAX+ 395, rustc 1.98.1.
"Spans" counts the highlight spans returned.

| Scenario             |    Size | Spans (unpatched) | Unpatched | Spans (patched) | Patched |
| -------------------- | ------: | ----------------: | --------: | --------------: | ------: |
| `blocks_32000`       |  64 KiB |            64 004 |   31.9 ms |          64 004 | 32.6 ms |
| `blocks_48000`       |  94 KiB |            65 538 |    6.56 s |          96 004 | 51.8 ms |
| `blocks_64000`       | 125 KiB |            65 538 |   15.64 s |         128 004 | 71.4 ms |
| `parens_64000`       | 125 KiB |           128 006 |   62.1 ms |         128 006 | 63.3 ms |
| `parens_80000`       | 156 KiB |           131 070 |    9.90 s |         160 006 | 79.8 ms |
| `macro_blocks_40000` |  78 KiB |            80 014 |    2.92 s |          80 015 | 82.4 ms |

On 0.25.10, with the patch every deep shape was linear (about 0.5–0.6 ms
per KiB) and complete; shapes inside the 16-bit range were unchanged. The
0.25.10 patch was the field width alone (hunk 1); hunks 2 and 3 were added
on this base.

Exit condition: drop hunks 1 and 2 together once an upstream release widens
`QueryState::start_depth` (or otherwise stops truncating the start depth)
without regressing optional-root patterns. Drop hunk 3 once upstream stops
matching DONE-step states against nodes. Keep the regression tests; they
must stay green on upstream's code.

## Base and maintenance

- Base: upstream tag `v0.27.2` (`3fb7be49`). `git log --oneline
  v0.27.2..HEAD` lists every carried commit.
- Keep each patch minimal and fenced. Do not edit outside the fences; a
  change that needs more belongs in a new, separately fenced patch with its
  own section here.
- Re-port onto a newer tag by rebasing the carried commits onto it on a new
  branch (never rewrite this one), e.g.
  `git rebase --onto vX.Y.Z v0.27.2 <new-branch>`. Resolve conflicts inside
  the fences only, then check each patch's exit condition against the new
  upstream and drop any patch upstream has made redundant.
- After a re-port, regenerate `lib/binding_rust/bindings.rs`
  (`cargo xtask generate-bindings`) if `api.h` changed, and run
  `cargo xtask fetch-fixtures`, `cargo xtask generate-fixtures`,
  `cargo test --workspace` (with and without `--features wasm`), and
  `cargo clippy --workspace --all-targets -- -D warnings`.
- REGISTRY-LANGUAGE needs a crates.io `tree-sitter-language` release that
  is byte-identical to the new base's `crates/language`; bump its version
  requirement to match.
- Update this file in the same change as any patch it describes.
