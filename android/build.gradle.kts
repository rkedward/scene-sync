// The Android Gradle plugin must sit on the same classloader as the Kotlin
// plugin, so it goes on the root buildscript classpath. It is skipped with
// -DengineOnly so the engine module can be tested without Google's Maven repo.
buildscript {
    if (System.getProperty("engineOnly") == null) {
        repositories {
            google()
            mavenCentral()
        }
        dependencies {
            classpath("com.android.tools.build:gradle:8.7.3")
        }
    }
}

plugins {
    kotlin("jvm") version "2.0.21" apply false
    kotlin("android") version "2.0.21" apply false
    kotlin("plugin.serialization") version "2.0.21" apply false
}
