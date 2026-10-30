import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// 版本號跟桌面版一致：根 Cargo.toml 的 [workspace.package] version
val appVersion: String = rootProject.file("../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }
    .substringAfter('"').substringBefore('"')
val (major, minor, patch) = appVersion.split('.').map { it.toInt() }
val abis: List<String> = (findProperty("meetscribe.abis") as String? ?: "arm64-v8a").split(',').map { it.trim() }

// 簽章：android\keystore.properties（不進版控）。沒有就產生未簽章的 release，交給 build-apk.ps1 處理。
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.isFile) f.inputStream().use { load(it) }
}

android {
    namespace = "io.github.win81134679.meetscribe"
    // Compose BOM 2026.09 的函式庫要求至少用 API 37 編譯；targetSdk 仍是 36（行為不變）
    compileSdk = 37

    defaultConfig {
        applicationId = "io.github.win81134679.meetscribe"
        minSdk = 26
        targetSdk = 36
        versionCode = major * 10000 + minor * 100 + patch
        versionName = appVersion
        // 原生函式庫只編 arm64（近年手機都是）；電腦上的模擬器測試加 -Pmeetscribe.abis=arm64-v8a,x86_64
        ndk { abiFilters += abis }
    }

    signingConfigs {
        if (keystoreProps.getProperty("storeFile") != null) {
            create("release") {
                storeFile = rootProject.file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
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
    }

    buildFeatures {
        compose = true
    }

    packaging {
        jniLibs { useLegacyPackaging = false }
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")
    implementation("androidx.core:core-ktx:1.19.1")
}

// 先交叉編譯 Rust 核心（android\build-rust.ps1），產生 jniLibs\arm64-v8a\libmeetscribe_android.so。
// 只改 Kotlin 時可加 -PskipRust 省時間（.so 要已經存在）。
val buildRust = tasks.register<Exec>("buildRust") {
    group = "build"
    description = "Cross-compile meetscribe-android (Rust) into jniLibs"
    workingDir = rootProject.projectDir
    commandLine(
        "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", rootProject.file("build-rust.ps1").absolutePath,
        "-Abi", abis.joinToString(","),
    )
}
if (!project.hasProperty("skipRust")) {
    tasks.named("preBuild") { dependsOn(buildRust) }
}
