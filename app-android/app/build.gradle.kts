plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Pip’s own Google sign-in identity, the phone half of the desktop’s google-client.json.
// Git-ignored: it identifies the app, users never see it, and an Android client carries no secret
// — Google checks the app’s signature instead.
val googleClientId: String = file("../google-client-android.json").takeIf { it.exists() }
    ?.readText()?.let { Regex("\"client_id\"\\s*:\\s*\"([^\"]+)\"").find(it)?.groupValues?.get(1) }
    ?: ""

android {
    namespace = "com.pip.companion"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.pip.companion"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
        // Real phones only for now; add x86_64 when something needs the emulator.
        ndk { abiFilters += listOf("arm64-v8a") }
        buildConfigField("String", "GOOGLE_CLIENT_ID", "\"" + googleClientId + "\"")
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    composeOptions { kotlinCompilerExtensionVersion = "1.5.14" }
    packaging { resources.excludes += setOf("/META-INF/{AL2.0,LGPL2.1}") }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2024.06.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.9.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.2")
    implementation("androidx.browser:browser:1.8.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.compose.ui:ui-tooling-preview")
}

// Pip's brain is the same Rust code the PC runs; cargo-ndk cross-compiles it and drops the .so
// straight into jniLibs, so there is no checked-in binary and no second implementation.
val rustAbis = listOf("arm64-v8a")

val buildRust by tasks.registering(Exec::class) {
    description = "Cross-compiles pip-jni for Android"
    workingDir = file("../rust")
    val args = mutableListOf("cargo", "ndk")
    rustAbis.forEach { args += listOf("-t", it) }
    args += listOf("-o", file("src/main/jniLibs").absolutePath, "build", "--release")
    commandLine(if (System.getProperty("os.name").startsWith("Windows")) listOf("cmd", "/c") + args else args)
}

tasks.named("preBuild") { dependsOn(buildRust) }
