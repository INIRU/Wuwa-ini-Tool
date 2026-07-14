# Security Policy

## Supported versions

Security fixes are provided for the latest published `1.x` release. Draft,
development, locally modified, and unofficially redistributed builds are not
supported release channels. Before the first verified public release, reports
against the default branch are still welcome.

## Report a vulnerability privately

Do **not** open a public Issue for a suspected vulnerability. Use GitHub's
[private vulnerability reporting form](https://github.com/INIRU/Wuwa-ini-Tool/security/advisories/new).
If that form is unavailable, contact the repository owner through a private
contact method listed on the owner's GitHub profile. Do not include secrets or
personal data in an unencrypted public message.

Please include:

- affected version or commit;
- Windows version and architecture;
- impact and realistic attack scenario;
- minimal reproduction steps or a proof of concept;
- whether user interaction or a malicious local file is required; and
- any suggested mitigation.

Particularly relevant areas include updater signature or release-chain bypass,
arbitrary file read/write/delete, path traversal or reparse-point handling,
unsafe profile/INI import, process identity confusion, unintended process
control, command execution, and exposure of local paths or private data.

You can expect an acknowledgement when a maintainer has access to the report.
Response and remediation times depend on severity and maintainer availability;
the project does not promise a fixed service-level agreement. Please allow a
reasonable coordinated-disclosure window before publishing details.

## Security boundaries

Wuwa ini Tool is designed not to inject code, access game memory, hook
anti-cheat, install a driver, change IFEO, or bypass game controls. A request to
add such behavior is outside scope, not a supported security feature.

Updater private keys and passwords must never be committed, printed in logs, or
shared in an Issue. Release automation must fail closed when protected signing
material, the updater public key, or required artifact verification is absent.

Ordinary game crashes, unsupported optimization claims, and option behavior
without a security impact belong in the bug or option-evidence form rather than
a private advisory.
