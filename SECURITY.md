# Security Policy

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting: open the **Security** tab of
this repository and choose **Report a vulnerability**. If that is unavailable for
you, open a minimal public issue asking for a private contact channel — do not
include the details in public.

Include what you found, how to reproduce it, the affected version or commit, and
your assessment of impact. You will get credit in the release notes unless you
prefer to stay anonymous.

## What matters in this project

API Watch parses untrusted OpenAPI and Swagger specifications. When testing, treat fixtures as hostile
input. Reports related to the following are especially welcome:

- path traversal or unintended file reads while scanning input
- code execution through crafted input
- anything that could leak secrets — API Watch is local-first: it must never send
  code, file contents, or environment data anywhere

## Supported versions

| Version | Supported |
| --- | --- |
| latest release | yes |
| older releases | best effort — please upgrade |

## Response timeline

- Acknowledgement: within 72 hours
- First assessment: within 7 days
- Fixes ship as a patch release as soon as they are ready.
