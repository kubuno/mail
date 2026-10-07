// The Kubuno mail client. A second app on the shared core; it consumes the
// mail module's HTTP API and reuses :core-account (accounts, sessions, the
// com.kubuno authenticator) and :core-ui (the design tokens) so it stays
// aligned with the drive app by construction.
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.ksp)
    alias(libs.plugins.hilt)
}

android {
    namespace = "com.kubuno.mail"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.kubuno.mail.android"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    // Release signing is opt-in: pass -PkubunoKeystore=… (and the passwords) to
    // sign, otherwise the release APK is unsigned (what the CI publishes). EVERY
    // Kubuno app MUST be signed with the SAME certificate — the shared-account
    // model grants access by matching signature.
    val keystorePath = (findProperty("kubunoKeystore") as String?)?.takeIf { it.isNotBlank() }
    val keystoreFile = keystorePath?.let { rootProject.file(it) }
    signingConfigs {
        if (keystoreFile != null && keystoreFile.exists()) {
            create("release") {
                storeFile = keystoreFile
                storePassword = findProperty("kubunoKeystorePassword") as String?
                keyAlias = findProperty("kubunoKeyAlias") as String?
                keyPassword = findProperty("kubunoKeyPassword") as String?
            }
        }
    }

    buildTypes {
        release {
            // R8 shrinks and obfuscates; proguard-rules.pro carries the keep-rules
            // audit (serializers, Retrofit/Room/Hilt/UnifiedPush reflection surfaces).
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
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
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(libs.kubuno.core.api)
    implementation(libs.kubuno.core.account)
    implementation(libs.kubuno.core.ui)
    implementation(libs.kubuno.core.viewer)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.appcompat)

    val composeBom = platform(libs.compose.bom)
    implementation(composeBom)
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    debugImplementation(libs.compose.ui.tooling)

    implementation(libs.activity.compose)
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.lifecycle.viewmodel.compose)

    implementation(libs.hilt.android)
    ksp(libs.hilt.compiler)
    implementation(libs.hilt.navigation.compose)

    implementation(libs.coroutines.android)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.retrofit.kotlinx.serialization)

    implementation(libs.room.runtime)
    implementation(libs.room.ktx)
    ksp(libs.room.compiler)

    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)

    // Push transport (UnifiedPush): receiver + register/unregister against a distributor.
    implementation(libs.unifiedpush.connector)

    testImplementation(libs.junit)
}
