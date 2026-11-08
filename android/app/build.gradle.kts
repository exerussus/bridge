import java.io.File
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Версия — как у моста (Cargo.toml workspace), чтобы телефон и ПК говорили об одной и той же версии.
val bridgeVersion: String = rootProject.file("../Cargo.toml").readLines()
    .first { it.trim().startsWith("version = ") }
    .substringAfter('"').substringBefore('"')
val vp = bridgeVersion.split('.').map { it.toIntOrNull() ?: 0 }
val bridgeCode = vp[0] * 1_000_000 + vp.getOrElse(1) { 0 } * 1_000 + vp.getOrElse(2) { 0 }

// Ключ подписи — вне проекта (C:\Exerussus\Programs\Bridge\keys), пути и пароли — в keys\most-android.properties
val signProps = Properties().apply {
    val f = rootProject.file("../../keys/most-android.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

// SDK — из local.properties (build-android.cmd пишет туда SDK Unity или Android Studio).
// compileSdk — самая новая установленная платформа (нужна 34+): в SDK Unity может не быть ровно 35.
val sdkDir: String? = Properties().apply {
    val f = rootProject.file("local.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}.getProperty("sdk.dir")
val bestSdk: Int = sdkDir?.let { d ->
    File(d, "platforms").listFiles()?.mapNotNull { it.name.removePrefix("android-").toIntOrNull() }?.maxOrNull()
} ?: 35
// build-tools — самые новые из установленных: AGP иначе полезет докачивать в SDK Unity (Program Files, без прав)
val bestTools: String? = sdkDir?.let { d ->
    File(d, "build-tools").listFiles()?.map { it.name }?.filter { it.first().isDigit() }
        ?.maxWithOrNull(compareBy<String>({ it.split('.').getOrNull(0)?.toIntOrNull() ?: 0 }, { it.split('.').getOrNull(1)?.toIntOrNull() ?: 0 }, { it.split('.').getOrNull(2)?.toIntOrNull() ?: 0 }))
}
check(bestSdk >= 34) { "В Android SDK нужна платформа 34 или новее (есть только android-$bestSdk)" }

android {
    namespace = "com.exerussus.most"
    compileSdk = bestSdk
    bestTools?.let { buildToolsVersion = it }
    // NDK — тот же, которым собрано ядро (build-android.cmd: -PmostNdkPath=…); нужен Gradle для очистки .so
    providers.gradleProperty("mostNdkPath").orNull?.let { ndkPath = it }

    defaultConfig {
        applicationId = "com.exerussus.most"
        minSdk = 26
        targetSdk = minOf(bestSdk, 35)
        versionName = bridgeVersion
        versionCode = bridgeCode
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    signingConfigs {
        if (signProps.getProperty("storeFile") != null) {
            create("most") {
                storeFile = rootProject.file("../../keys/" + signProps.getProperty("storeFile"))
                storePassword = signProps.getProperty("storePassword")
                keyAlias = signProps.getProperty("keyAlias")
                keyPassword = signProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("most")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }
    packaging { jniLibs { useLegacyPackaging = false } }
}

dependencies {
    val bom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(bom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    // привязки к ядру (uniffi) работают через JNA
    implementation("net.java.dev.jna:jna:5.15.0@aar")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
