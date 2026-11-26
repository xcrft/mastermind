# Sensitive Data Best Practices

Secret and personal-data handling rules shared by every source language.
Configuration, container, and pipeline files have their own secret rules.

## Enforceable rules

### sensitive-data-no-secrets-in-source

**Name:** No secrets in source

**Rule:** Do not commit passwords, API keys, tokens, account keys, private
keys, or credential-bearing connection strings in source. Read them from the
repository's secret store, managed identity, or environment.

**Why:** A committed secret is compromised for every clone and stays in history
after removal.

**Detect:** A diff assigns a literal that is itself credential-shaped
(`Password=`/`Pwd=` in a connection string, `AccountKey=`, a known provider
prefix, private-key material, or a high-entropy token) in production code, or
the surrounding code proves that a credential-bearing value is hard-coded.
An identifier such as `token`, `*_key`, or `connectionString` and a non-empty
literal are not sufficient evidence by themselves: parser tokens, object keys,
cancellation tokens, public identifiers, and obvious test fixtures are not
secrets.

## Advisory rules

### sensitive-data-no-secrets-or-pii-in-logs

**Name:** No secrets or PII in logs, telemetry, or errors

**Rule:** Do not write secrets, credentials, raw user text, or PII (user names,
emails, IPs, device, machine, or file identifiers) to logs, telemetry, console
output, or exception messages. Log opaque ids, counts, status, and error
objects. Types holding secrets or user text should redact in their
`Debug`/`ToString`/`__repr__` output instead of deriving it.

**Why:** Log and telemetry sinks are shipped, retained, and broadly readable,
bypassing every in-application protection.

**Detect:** A diff passes a field named like a secret or PII (`password`,
`token`, `apiKey`, `connectionString`, `email`, `userName`, `ip`, `prompt`,
`text`), or a whole request, response, message, or user object, to a log,
telemetry, console, or exception-message call; or derives `Debug` on a type
with a secret-named field.
