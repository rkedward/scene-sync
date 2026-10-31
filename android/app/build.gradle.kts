plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
}

android {
    namespace = "dev.scenesync.player"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.scenesync.player"
        // The HY300 Pro+ runs Android 14 (API 34). 26 keeps older projectors possible.
        minSdk = 26
        targetSdk = 34
        // Minutes since 2026-01-01 at build time: every newer build (CI, release
        // or local) has a higher number, so it can replace an older one.
        versionCode = (System.currentTimeMillis() / 60_000 - 29_453_760).toInt()
        versionName = System.getenv("SCENESYNC_VERSION") ?: "0.5.4"
    }

    signingConfigs {
        // A fixed debug key checked into the repo, so every build (CI or local)
        // has the same signature and can update the app in place. It protects
        // nothing; use a private release key for anything public.
        getByName("debug") {
            storeFile = file("debug.keystore")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Sideloaded builds: sign with the debug key until there is a release key.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        buildConfig = true
    }
}

dependencies {
    implementation(project(":engine"))

    val media3 = "1.5.1"
    implementation("androidx.media3:media3-exoplayer:$media3")
    implementation("androidx.media3:media3-ui:$media3")

    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.activity:activity-ktx:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
}
