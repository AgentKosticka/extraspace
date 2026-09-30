plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "io.github.tymonoman.extraspace"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.tymonoman.extraspace"
        // The host refuses to talk to a mismatched app, so this is the number it
        // compares against when deciding whether to push a new APK.
        versionCode = rootProject.file("../companion-version").readText().trim().toInt()
        versionName = "0.1.0"
        // MediaFormat.KEY_LOW_LATENCY needs 30; below that the decoder buffers
        // several frames and the whole latency budget is gone.
        minSdk = 30
        targetSdk = 35
    }

    // CI restores a persistent key from a repository secret. Local source builds
    // keep using the developer's debug key unless an explicit key is supplied.
    val releaseKeystore = System.getenv("EXTRASPACE_KEYSTORE")
    if (releaseKeystore != null) {
        signingConfigs.create("published") {
            storeFile = file(releaseKeystore)
            storePassword = System.getenv("EXTRASPACE_KEYSTORE_PASSWORD") ?: "android"
            keyAlias = System.getenv("EXTRASPACE_KEY_ALIAS") ?: "androiddebugkey"
            keyPassword = System.getenv("EXTRASPACE_KEY_PASSWORD") ?: "android"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName(if (releaseKeystore != null) "published" else "debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    sourceSets["main"].java.srcDirs("src/main/kotlin")
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.activity:activity:1.9.3")
    implementation("androidx.appcompat:appcompat:1.7.0")
}
