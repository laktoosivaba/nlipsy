# Security policy

## Supported versions

The latest release. The six published crates share one 0.x version and ship together,
and fixes go into a new release rather than back onto an older line.

## Reporting a vulnerability

Report privately, through GitHub security advisories:
<https://github.com/laktoosivaba/nlipsy/security/advisories/new>.

Please do not open a public issue for a vulnerability first — an advisory is private
until it is published, which leaves room to prepare a release alongside the
disclosure.

Useful in a report: the version, a message or byte sequence that reproduces the
problem, and what it causes (panic, unbounded allocation, a message accepted that
the standard says must be rejected, or the reverse).
