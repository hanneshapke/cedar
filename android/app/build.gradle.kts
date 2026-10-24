import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// FCM needs the Firebase project's google-services.json (a repository secret in CI, your own
// project's file locally). Without it the app builds all the same, with only UnifiedPush (ntfy).
val hasGoogleServices = file("google-services.json").exists()
if (hasGoogleServices) apply(plugin = libs.plugins.google.services.get().pluginId)

// The version is the release's: `-Pcedar.version=1.2.3` in CI, else the workspace version in the
// repository's Cargo.toml, so the APK and the desktop app always carry the same number.
val cedarVersion: String = (findProperty("cedar.version") as String?)
    ?: Regex("""\[workspace\.package\][^\[]*?version\s*=\s*"([^"]+)"""")
        .find(rootDir.resolve("../Cargo.toml").readText())?.groupValues?.get(1)
    ?: "0.0.0"

/** major·1 000 000 + minor·1 000 + patch: higher for every release, so installed apps update. */
fun versionCodeOf(version: String): Int {
    val (major, minor, patch) = version.substringBefore('-').split('.').map { it.toIntOrNull() ?: 0 }.plus(listOf(0, 0, 0)).take(3)
    require(minor < 1000 && patch < 1000) { "version $version doesn't fit the versionCode scheme" }
    return major * 1_000_000 + minor * 1_000 + patch
}

// Release signing: a keystore named by CEDAR_KEYSTORE (or android/keystore.properties). Without one,
// the release APK is left unsigned and build-apk.sh says so.
val keystoreProps = Properties().apply {
    rootDir.resolve("keystore.properties").takeIf { it.exists() }?.inputStream()?.use { load(it) }
}
fun signingValue(env: String, prop: String): String? = System.getenv(env)?.takeIf { it.isNotEmpty() } ?: keystoreProps.getProperty(prop)
val keystorePath = signingValue("CEDAR_KEYSTORE", "storeFile")

android {
    namespace = "dev.cedar.cedar"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.cedar.cedar"
        minSdk = 26
        targetSdk = 35
        versionName = cedarVersion
        versionCode = versionCodeOf(cedarVersion)
        buildConfigField("boolean", "HAS_FIREBASE", hasGoogleServices.toString())
    }

    signingConfigs {
        if (keystorePath != null) {
            create("release") {
                storeFile = file(keystorePath)
                storePassword = signingValue("CEDAR_KEYSTORE_PASSWORD", "storePassword")
                keyAlias = signingValue("CEDAR_KEY_ALIAS", "keyAlias")
                keyPassword = signingValue("CEDAR_KEY_PASSWORD", "keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
        }
        debug {
            // Same application id as the release, so one google-services.json covers both.
            versionNameSuffix = "-debug"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    lint {
        abortOnError = true
        checkReleaseBuilds = false
        // Dependency updates are a decision, not a lint failure.
        disable += setOf("GradleDependency", "NewerVersionAvailable", "AndroidGradlePluginVersion", "OldTargetApi")
    }

    packaging {
        resources.excludes += setOf("/META-INF/{AL2.0,LGPL2.1}", "META-INF/versions/9/OSGI-INF/MANIFEST.MF")
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(project(":core"))

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.process)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.kotlinx.coroutines.android)

    implementation(libs.coil.compose)
    implementation(libs.osmdroid)
    implementation(libs.zxing.embedded)
    implementation(libs.unifiedpush.connector)
    implementation(platform(libs.firebase.bom))
    implementation(libs.firebase.messaging)
    implementation(libs.play.services.base)
}
