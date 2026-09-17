# Security policy

## Supported versions

Security fixes are released for the latest version of ChromaFree only. Please update before reporting.

## Reporting a vulnerability

Please do not report security vulnerabilities in public issues, pull requests or on Discord.

Report them privately through
[GitHub private vulnerability reporting](https://github.com/MakotoPD/ChromaFree/security/advisories/new). Include:

- the affected version and Windows build,
- a description of the vulnerability and its impact,
- steps to reproduce or a proof of concept.

You should receive a response within 7 days. Once the issue is confirmed, a fix is prepared and released, and the
advisory is published with credit to the reporter unless you prefer to stay anonymous.

## Scope

ChromaFree installs a virtual camera source (`vcam-source.dll`) that Windows loads into the Frame Server service and
shares frames with the app through named shared memory. Reports about these components, the installer and the
handling of files such as background images and configuration are especially welcome.
