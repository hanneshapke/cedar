// Where the page sends contributions: a cedar-contrib ingest (contrib/). The build
// (contrib-web/build.sh) rewrites this line from CONTRIB_INGEST_URL; this default is the
// project's own ingest, the one cedar-chat sends to (DEFAULT_CONTRIBUTIONS_URL in chat/src/config.rs).
// That ingest must list this page's origin in CONTRIB_CORS_ORIGINS. An empty string means the
// page's own origin, for an ingest that serves the page itself.
window.CEDAR_CONTRIB_CONFIG = { ingest: 'https://contrib-api-7fdw4asdra-uw.a.run.app' };
