pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "cedar-android"

// `core` is plain Kotlin (the /api types, the transcript reducer, the SSE parser, Markdown, cards,
// attachments rules, the HTTP client) and builds without the Android SDK; `app` is the Android app.
include(":core", ":app")
