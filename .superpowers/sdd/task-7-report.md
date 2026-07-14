# Task 7 implementation report

## Scope

- Windows CPU Set, processor-group hard-affinity compatibility path, six process priority classes, readback, and stable error mapping.
- Default-off Focus Mode with read-only preview, hard communication/capture protections, exact priority recovery journal, adaptive contention/hot-thread policy, bounded telemetry thresholds, and Windows PDH plus process/thread-time sampling.
- Explicit opt-in WuWa execution-speed QoS normalization with readback and a serializable exact-restore record.
- Windows fixture coverage that never applies Realtime and only mutates a disposable copied executable.

## Rules read

- `/Users/iniru/.codex/agents/rules/codexthink-core.md`
- `/Users/iniru/.codex/agents/rules/backend-db-security.md`
- `/Users/iniru/.codex/agents/rules/verification.md`
- `superpowers:test-driven-development`

## TDD evidence

- Process-control compile stubs: 12 tests executed with 9 expected assertion failures before implementation.
- CPU Set buffer parser stubs: 2 expected assertion failures before implementation; 3 parser tests passed after implementation.
- Focus Mode stubs: 11 tests executed with 10 expected assertion failures before implementation.
- Adaptive policy stubs: 2 expected assertion failures before implementation.
- QoS public model RED: unresolved imports for `GameQosRequest`, `GameQosState`, `GameQosRestoreGuard`, and `classify_game_qos_restore`.
- Bounded telemetry/preview RED: missing `FocusPreview::thresholds` and `FocusThresholds::bounded`.

## Verification

- `cargo test --manifest-path src-tauri/Cargo.toml --test process_control --test focus_mode --test game_qos`: passed, 40 tests.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`: passed.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: passed.
- `git diff --check`: passed.
- `cargo clippy --manifest-path .superpowers/sdd/windows-probe/Cargo.toml --target x86_64-pc-windows-msvc -- -D warnings`: passed. This excluded diagnostic probe compiles the actual Windows process, Focus, telemetry, WASAPI, PDH, and QoS modules against the MSVC target.
- Full `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc`: environment-blocked before crate typechecking because `ring` cannot find the MSVC `assert.h`; Tauri also reports the not-yet-created `icons/icon.ico` resource.
- Full host `cargo test --manifest-path src-tauri/Cargo.toml`: passed, 176 tests across the library and integration suites.

## Safety notes

- Every process mutation revalidates the exact PID/path; Focus restore additionally requires process creation time and current app-applied value.
- QoS restore records include PID, creation time, canonical game image, prior state, and applied state; malformed or cross-image records are rejected before backend access.
- CPU Sets remain the recommended soft selection. Hard affinity is explicit, single-group-only, nonzero, and mask-validated.
- Focus selection alone performs no mutation. Priority reduction occurs only on a sustained aggregate-contention decision and only changes Normal to Below Normal.
- High and Realtime remain explicit and acknowledgement-gated; neither is used by the Windows fixture.
- Telemetry uses language-neutral English PDH counter paths plus process/thread time deltas and never reads game memory.
- QoS recovery state is written to a versioned, atomic, durable journal before the execution-speed mutation; journal failure prevents mutation.
- Adaptive priority targets require a sustained per-process competitor streak. Per-logical saturation remains explanatory unless the measured WuWa thread is also sustained-hot.
- Main-thread Headroom is deliberately advisory in this task (`RecommendSoftCpuSets`); no background CPU Set mutation is claimed or performed without a future session supervisor and matching recovery implementation.

## Review disposition

- Mandatory read-only review initially found no Critical issues and eight Important issues.
- Fixed per-process restraint targeting, sustained/exact hot-thread classification, independent release hysteresis, pre-mutation QoS journaling, validated-game constructor boundaries, process-family protection, and added a Windows Focus fixture.
- The soft CPU Set Headroom result remains explicitly advisory because REQ-CPU-013 says it “may” act; shipping an unjournaled mutation would be less safe than returning a typed recommendation.
- Final read-only re-review found no remaining Critical or Important issues and returned `Ready to merge: Yes`.
