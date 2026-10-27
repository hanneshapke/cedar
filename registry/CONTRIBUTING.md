# Contributing a tool

Thanks for sharing a tool! Each tool is one folder in `registry/tools/`, added or updated by pull
request. The pull request is the review: a maintainer reads the code, and CI checks the rules.

## 1. Write the tool

A tool is a Python script that reads its input as JSON on stdin and prints its result on stdout.
Exiting non-zero with a message on stderr reports an error to the model. See [Personal tools in
Python](../README.md#personal-tools-in-python) for the details, and
[tandem_glucose](tools/tandem_glucose) for a full example. To show charts, print `[chart] {spec}`
lines (the spec is `generate_chart`'s input without a path); the chat draws them as chart cards.

- Test it locally first: put the folder in your own `TOOLS_DIR` and run cedar.
- Read credentials from environment variables, and declare each one in `secrets`. Never put a
  credential in a file. A `.env.example` that documents the variables is fine.
- Use the standard library, what the sandbox image ships, or packages listed in `pip`. Don't
  install anything at run time; cedar installs the `pip` list when the tool is installed.
- Only reach the hosts you list in `network`.
- Write only inside `/workspace`, and only what the user asked for.

## 2. Write the manifest

```json
{
  "description": "What it does and when the model should use it, and how to present the result.",
  "version": "1.0.0",
  "author": "Your Name",
  "homepage": "https://example.com/where-to-report-issues",
  "license": "Apache-2.0",
  "input_schema": { "type": "object", "properties": { "days": { "type": "integer" } } },
  "network": ["api.example.com"],
  "secrets": [
    "EXAMPLE_TOKEN",
    { "name": "EXAMPLE_REGION", "description": "us or eu", "optional": true }
  ],
  "pip": ["requests>=2.31"],
  "health_data": false
}
```

The rules (CI checks them, and so does cedar):

- The folder name is the tool name: lowercase letters, digits and `_`, and not one of cedar's own
  tools or groups.
- `version` follows `MAJOR.MINOR.PATCH`. Bump it with every change.
- `author` and `license` are required. Use an open source license.
- `network`: host names, or `*.domain` for a domain's subdomains. At most 20.
- `secrets`: `UPPER_CASE` names. `PATH`, `HOME`, `PYTHON*`, `PIP_*`, `LD_*`, `CEDAR_*` and
  similar are reserved.
- `pip`: plain requirements such as `name>=1.0`. No options, URLs, or local paths.
- Plain file names only: no subfolders, no hidden files except `.env.example`. At most 50 files,
  2 MiB each, 8 MiB in all.

Add a `listing.json` next to it for the registry:

```json
{ "summary": "One line, at most 120 characters", "category": "productivity" }
```

Categories: `health`, `productivity`, `data`, `finance`, `home`, `media`, `developer`, `other`.
Only a maintainer adds `"reviewed": true`.

### Translations

cedar's clients are in English, German and French. Two kinds of text in a tool are for people, and
can be translated; the rest stays in English:

- **For the model: English.** `description` and the `input_schema` descriptions in `tool.json` are
  instructions to the model, like cedar's system prompt. The model answers in the user's language
  whatever language they are in.
- **For the admin's tool list: `translations` in `listing.json`.** A one-line `summary` (at most 120
  characters) and descriptions of the secrets `tool.json` declares, per locale (`de`, `fr`, or one
  with a region such as `de-CH` where it differs):

  ```json
  {
    "summary": "Air quality, pollutants, UV and pollen from Open-Meteo",
    "category": "health",
    "translations": {
      "de": { "summary": "Luftqualität, Schadstoffe, UV und Pollen von Open-Meteo" },
      "fr": { "summary": "Qualité de l'air, polluants, UV et pollens d'Open-Meteo",
              "secrets": { "EXAMPLE_TOKEN": "Votre jeton d'API" } }
    }
  }
  ```

  Translations are only shown, never part of what an admin accepts, so they don't change the
  tool's permissions. They are kept out of `tool.json` because older cedar servers refuse a manifest
  with fields they don't know.
- **For the user, at run time: `CEDAR_LANGUAGE`.** Every call gets `CEDAR_LOCALE` (`de-CH`) and
  `CEDAR_LANGUAGE` (`de`, `fr` or `en`); either may be missing on older servers, so default to English.
  Text the user sees directly, above all `[chart]` titles, series names and axis labels, should follow
  it when the tool can: a small `STRINGS = {"en": {...}, "de": {...}, "fr": {...}}` table is
  enough, with no extra packages. Write weekday and month names from your own table rather than
  `strftime`, since the sandbox has no locales installed. Numbers in a chart stay numbers: the chat
  formats them for the user. Text for the model can stay in English.

Use the terms and tone in [docs/i18n-glossary.md](../docs/i18n-glossary.md) (German *du*, French
*vous*).

## 3. Build the index and open a pull request

```sh
python3 registry/build_index.py          # updates registry/index.json
python3 registry/build_index.py --check  # what CI runs
cargo test --test registry               # cedar's own checks on every tool
```

Commit the tool folder and `index.json` together.

## Health tools

Tools that read or return personal health data set `"health_data": true`. They are welcome, with
extra rules:

- **No dosing.** A tool must never compute or suggest medication or insulin doses, basal rates,
  carb ratios, correction factors, or any change to a device's settings. Tools like that may fall
  under medical-device rules (such as FDA software-as-a-medical-device guidance or the EU MDR),
  and they will not be accepted.
- Put safety first: report lows and other risks before anything else.
- Say in the description that the result is informational, and point to the user's care team for
  changes. cedar adds its own health rules to the model's instructions too.
- Send the data only to the hosts you declare, and keep nothing outside `/workspace`.
- A maintainer reviews every release of a health tool before it is listed (`"reviewed": true`).

## Updating and removing

Change the files, bump `version`, rebuild the index, and open a pull request. Servers that have
the tool installed see the update in their settings, and installing it asks again for the
permissions. To remove a tool from the registry, delete its folder and rebuild the index; servers
keep their installed copy until an admin uninstalls it.

## Reporting a problem

If a tool misbehaves or looks unsafe, open an issue naming the tool and version. A maintainer can
remove it from the index at once.
