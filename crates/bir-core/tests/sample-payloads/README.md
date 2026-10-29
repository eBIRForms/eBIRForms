# Sample payloads (dummy data)

These are what our serializers produce for a fictitious taxpayer (TIN
123-456-788-00000). The submission queue hands exactly these bytes to its
transport: `background_cron` builds them with `try_to_bir_xml_payload`.
Encrypting them into the IAF upload format happens server-side, not in this
repo.

| File | Contents |
|---|---|
| `*.plain.xml` | Plaintext BIR pseudo-XML: `<div>key=valuekey=</div>` per field |
| `*.official.xml` | What the official eBIRForms page submits for `*.steps.json` |
| `*.steps.json` | The filer's input, replayed against the official page |

`tests/sample_payloads_test.rs` fails if a serializer change alters these
files. If the change is intended, regenerate them with:

```sh
UPDATE_SAMPLE_PAYLOADS=1 cargo test -p bir-core --test sample_payloads_test
```

`*.official.xml` is what the official eBIRForms page submits when a filer
enters `*.steps.json` (the official HTA run with its own scripts in jsdom, so
the official code computes and formats every amount, e.g. `250,000.00`). The
test requires `*.plain.xml` to equal it, which checks calculations,
formatting and serialization together.

The plaintext follows each form's official layout
(`crates/bir-core/data/official-xml/<form_id>.json`): page order, `escape()` on
the fields the official code escapes, 2551Q's ten fields written twice,
1601C's Address 2 folded into `txtAddress`, the form's whitespace and the
`All Rights Reserved BIR 2012.0` trailer.
