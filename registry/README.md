# Community tool registry

Tools for [cedar](../README.md) that people share with each other. They are **not officially
supported**: the people who write them maintain them, and a listing here means a tool follows the
rules below, not that anyone vouches for what it does. Health tools in particular are
informational, not medical devices.

A cedar server installs tools from here once an admin points it at the index:

```sh
TOOLS_DIR=$HOME/.cedar/tools
TOOLS_REGISTRY_URL=https://raw.githubusercontent.com/<owner>/cedar/main/registry/index.json
```

Admins then browse, install, update and uninstall tools from the chat's settings (Integrations →
Community tools) or with the admin API (see [the README](../README.md#community-tools)).

## How a tool is kept safe

- **Pinned files.** `index.json` lists the SHA-256 of every file of every tool. cedar downloads
  the files and refuses any that differ, so what runs is what was reviewed here.
- **Declared permissions.** A tool's `tool.json` says which hosts it reaches (`network`), which
  credentials it reads (`secrets`), which packages it needs (`pip`), and whether it handles
  health data (`health_data`). The admin sees and accepts exactly these before installing, and
  cedar refuses a release whose manifest asks for more than its listing.
- **Isolation.** Every call runs in a container of its own that sees only that tool's folder, the
  run's workspace, and the tool's own package environment. It gets a network only if it declares
  hosts, and only the secrets it declares, which admins save in cedar (encrypted), never in files.
- **Approval.** In interactive runs, every call of a community tool waits for the user's OK until
  they allow that release always; an update asks again.
- **Health rules.** When a tool declares `health_data`, cedar adds rules to the model's
  instructions, whatever the tool's description says: no dosing advice, safety first, and point to
  the care team.

One limit to know: the `network` hosts are shown and approved, but they are not yet enforced host
by host. A tool that declares any host gets the network `TOOLS_NETWORK` gives it (`bridge` by
default). That is why reviews read the code.

## Layout

```
registry/
├── index.json            generated: python3 registry/build_index.py
├── build_index.py        builds the index, or checks everything with --check (CI runs it)
└── tools/
    └── tandem_glucose/
        ├── tool.json     the manifest cedar reads
        ├── tool.py       the script
        ├── .env.example  optional: documents the settings for local use
        └── listing.json  for the registry only: summary, category, reviewed
```

See [CONTRIBUTING.md](CONTRIBUTING.md) to add or update a tool.
