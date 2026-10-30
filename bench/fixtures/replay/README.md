# Recorded answers

`/replay/<service>/<path>` answers from the YAML files in `<service>/` here: the first file whose
`method`, `path` and `query` (a subset of the request's) match. Use it for services too intricate
to synthesize, such as the MOTIS trip planner behind `plan_trip` (`transit`) and OSRM (`driving`).

To record, run the fixture server with `--record transit=https://api.transitous.org` and make the
requests (for example, by running the task once). Each miss is forwarded upstream and saved here.
Check recordings for anything personal before committing them.

```yaml
method: GET
path: api/v1/geocode
query: {text: Port Alder}
status: 200
content_type: application/json
body: {...}
```
