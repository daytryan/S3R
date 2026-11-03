# Security

## Reporting a problem

Please report security problems privately, not in a public issue or on the Discord. On GitHub, open the
repository's **Security** tab and press **Report a vulnerability**
([direct link](https://github.com/daytryan/S3R/security/advisories/new)). Say what is affected, how to
reproduce it and what an attacker could do. The answer comes there too. Please give us time to fix it before you
share the details anywhere.

Examples of what counts: a way to make the launcher install a file that is not from a signed release, or a way for
a `.skate` map or another player in an online session to run code on your PC or read your files.

## How updates are protected

- The launcher installs only signed releases. Every release has a `release.json` that lists each file with its
  SHA-256, and a `release.json.sig`: an ed25519 signature of that file made with the owner's release key, which is
  kept offline and never in this repository.
- The launcher checks the signature with the public key built into it (`RELEASE_KEY` in
  `crates/skate-launcher/src/update.rs`) and refuses a release whose signature is missing or wrong. It then checks
  each download against the SHA-256 in the signed `release.json`.
- A suspected leak of the release key is a security problem: report it as above.

## Supported versions

Only the newest release gets fixes. The launcher offers each update when it opens.
