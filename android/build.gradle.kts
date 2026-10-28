// The Android plugin version is set in settings.gradle.kts so that the
// engine module can build without it (see the engineOnly property there).
plugins {
    kotlin("jvm") version "2.0.21" apply false
    kotlin("android") version "2.0.21" apply false
    kotlin("plugin.serialization") version "2.0.21" apply false
}
