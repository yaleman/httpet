# httpet

HTTP status pets for httpet.org and subdomains like dog.httpet.org.

## Admin access

The `/admin` routes are expected to be protected by external authentication
routes or proxy auth before requests reach the app. The app still restricts
admin pages to the configured base domain and requires CSRF tokens on admin
mutations, but it does not implement an in-app admin login.

## OpenAI image generator

The `openai_image_generator` CLI (`src/bin/openai_image_generator.rs`) uses the OpenAI Images API (generations endpoint) to create square images and save them to `images/<animal>/<code>.png`. The script requests a 1024x1024 image and expects base64 image data in the response, writing a PNG file on disk.

Usage:

```bash
OPENAI_API_KEY=your_key_here \
  cargo run --bin openai_image_generator -- dog --code 404
```

Options:

- `--model` (defaults to the script's configured model; see the OpenAI Images API docs for available choices).
- `--quality` (`auto`, `low`, `medium`, `high`) for GPT image models.
- `--code` to target a specific HTTP status code.

The OpenAI API uses API keys for authentication. Keep your key out of source control and load it from the `OPENAI_API_KEY` environment variable.

For supported models, parameters, and response formats, see the OpenAI Images API docs.
