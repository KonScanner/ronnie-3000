# Security policy

## Report a vulnerability

Please do not open a public issue for a security problem. Use
[private vulnerability reporting](https://github.com/KonScanner/ronnie-3000/security/advisories/new)
on GitHub instead. Include the steps to reproduce the problem and the version that you use.

You will get an answer within seven days.

## What Ronnie protects

- Your TONE3000 API key and sign-in tokens stay on your computer. On Linux and macOS, only your
  user can read their files. Ronnie sends tokens only to the TONE3000 origin over HTTPS.
- Once a day, Ronnie asks `api.github.com` if a new release exists. That request carries no
  account data and no key. Turn it off in Settings.
- Model and image downloads go only to HTTPS hosts on an allow list, and they never carry your
  token.
- Downloaded models and impulse responses are data. Ronnie never runs code from them.

## Supported versions

Only the latest release gets security fixes.
