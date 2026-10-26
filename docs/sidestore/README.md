# The SideStore source

`sidestore.json` on the project's GitHub Pages site
(`https://hanneshapke.github.io/cedar/sidestore.json`, or the address Settings → Pages shows) is a
[SideStore](https://sidestore.io) and AltStore source for the iOS app. Add it in SideStore under
**Sources → +**, and Cedar shows up to install and to update when a release comes out. See
[iOS/README.md](../../iOS/README.md#sideloading) for what the sideloaded app can and can't do.

## How it is built

| Piece | What it does |
| --- | --- |
| `.github/workflows/ipa.yml` | Builds `Cedar-<version>-ios.ipa` and next to it `Cedar-<version>-ios.source.json`: the bundle id, version, build number, minimum iOS, size, SHA-256 and privacy strings, read from the built app's `Info.plist` |
| `.github/workflows/release.yml` | Attaches both files to the GitHub Release, then starts `pages.yml` |
| `.github/workflows/pages.yml` | Fetches the `.source.json` of every published release that has an IPA, adds its download URL, date and a link to its notes, and runs `build.sh` |
| `source.json` | Everything else: the source's and the app's name, description, icon, tint color, category and screenshots. URLs without `https://` are relative to the site |
| `build.sh` | Puts the two together: the versions newest first, the newest one's privacy strings as the app's permissions, and the app-level `version`/`downloadURL`/`size` older clients read |

The entries come from the built app rather than from the project, because SideStore refuses an
install when the IPA's version, build number or usage descriptions differ from what the source
says. Until a release has an IPA, the source has no apps.

To edit the app's listing, change `source.json`; a push to main that touches `docs/sidestore`
republishes the site. To see the result locally, with the version files of a few releases in a
directory:

```sh
docs/sidestore/build.sh https://hanneshapke.github.io/cedar/ path/to/versions | jq .
```
