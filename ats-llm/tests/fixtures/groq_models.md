# Rate Limits

| MODEL ID | RPM | RPD | TPM | TPD | ASH | ASD |
|---|---|---|---|---|---|---|
| openai/gpt-oss-120b | 30 | 1K | 8K | 200K | - | - |
| whisper-large-v3-turbo | 20 | 2K | - | - | 7.2K | 28.8K |

## Rate Limit Headers

| Header | Value | Notes |
|---|---|---|
| retry-after | 2 | In seconds |
