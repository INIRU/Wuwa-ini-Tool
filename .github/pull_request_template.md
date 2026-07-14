## Summary

<!-- Explain the user-visible outcome and why this change is needed. -->

## Related Issue

<!-- Use "Closes #123" when appropriate. -->

## Verification

<!-- List exact commands, Windows integration evidence, and manual checks. -->

- [ ] `node scripts/check-version.mjs`
- [ ] `node scripts/validate-catalog.mjs`
- [ ] Frontend tests, typecheck, and build passed when affected.
- [ ] Rust format, clippy with warnings denied, and tests passed when affected.
- [ ] Windows/Tauri behavior was verified when affected.

## Safety, privacy, and recovery

<!-- Describe trust boundaries, confirmation, backup/restore, failure behavior, and data handling. -->

- [ ] No injection, game-memory access, anti-cheat hook, driver, IFEO, bypass, or unrestricted file/process control was added.
- [ ] Untrusted input is validated in Rust at the trust boundary.
- [ ] Secrets, account identifiers, personal paths, and private logs are excluded.
- [ ] User data and external changes are preserved or a concrete recovery path is documented.

## Catalog evidence (if applicable)

<!-- Include game version, launcher, CPU/GPU/RAM, exact option and before/after values, FPS/1% low method, area/duration, artifacts, crashes, and logs. -->

- [ ] Evidence state and runtime-verification claims are accurate.
- [ ] No unlicensed or GPL-incompatible reference content was copied.

## Risk and rollback

<!-- State residual risks, downgrade/rollback behavior, and anything reviewers must verify. -->

## Contributor checklist

- [ ] The change is focused and does not revert unrelated work.
- [ ] Public contracts and non-obvious safety decisions have concise English comments.
- [ ] Tests cover the behavior or the reason tests are not possible is documented.
- [ ] User-facing Korean and English copy are both updated when affected.
- [ ] Documentation and changelog impact were considered.
