# Privacy policy translations

[`PRIVACY.md`](../../PRIVACY.md) at the repository root is the privacy policy. It is the English
original and the version that applies. This folder holds its translations, one file per language the
website speaks: `PRIVACY.de.md`, `PRIVACY.fr.md` and `PRIVACY.es.md`.

The website's privacy page (`docs/landing/privacy.html`) shows the visitor's language when its
translation matches today's `PRIVACY.md`, and English otherwise.

## The first line

Each translation starts with the version of `PRIVACY.md` it was translated from, as the first 12
characters of that file's SHA-256:

```
<!-- translated-from: PRIVACY.md 3c80f87a1240 -->
```

## Changing the policy

CI (`node scripts/check-i18n.js`) fails while a translation was made from an older `PRIVACY.md`, so a
pull request that changes the policy updates its translations too:

1. Change `PRIVACY.md`.
2. Run `node scripts/check-i18n.js`. For each translation it names the commit to diff against
   (`git diff <commit> -- PRIVACY.md`) and the new first line.
3. Bring each translation up to date with that diff, then set its first line to the one the check
   printed. Keep the date in *Last updated* the same as the English one.

The check also fails when a translation has a different number of headings than `PRIVACY.md`, or
different `code`: settings, paths and commands are never translated.

## Style

Follow [`docs/i18n-glossary.md`](../i18n-glossary.md): German says *du*, French *vous*, Spanish *tú*.
Name settings the way the apps label them (Einstellungen → Gedächtnis, Réglages → Mémoire,
Configuración → Memoria). Each translation keeps the note under its title that the English version
applies.
