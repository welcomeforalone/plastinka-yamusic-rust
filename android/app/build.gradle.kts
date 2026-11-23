import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

android {
    namespace = "ru.plastinka"
    compileSdk = 35

    defaultConfig {
        applicationId = "ru.plastinka"
        minSdk = 26
        targetSdk = 35
        versionCode = 5
        versionName = "0.0.5"
    }

    // Подпись release: ключ в репозитории не хранится.
    // Источник — переменные окружения ANDROID_KEYSTORE_* или файл android/keystore.properties.
    val releaseSigning: Map<String, String>? = run {
        fun need(values: Map<String, String?>, field: String, source: String): String {
            val value = values[field]?.trim()
            if (value.isNullOrEmpty()) {
                throw GradleException("Подпись release: не задано значение «$field» ($source).")
            }
            return value
        }

        // Путь к хранилищу проверяем сразу после его чтения: ошибка про файл понятнее, чем про пароли.
        fun needKeystore(values: Map<String, String?>, field: String, source: String): String {
            val path = need(values, field, source)
            if (!rootProject.file(path).isFile) {
                throw GradleException("Подпись release: файл хранилища ключей не найден: $path ($source).")
            }
            return path
        }

        val envPath = System.getenv("ANDROID_KEYSTORE_PATH")
        val propsFile = rootProject.file("keystore.properties")
        if (!envPath.isNullOrBlank()) {
            val source = "переменные окружения ANDROID_KEYSTORE_*"
            val env = mapOf(
                "ANDROID_KEYSTORE_PATH" to envPath,
                "ANDROID_KEYSTORE_PASSWORD" to System.getenv("ANDROID_KEYSTORE_PASSWORD"),
                "ANDROID_KEY_ALIAS" to System.getenv("ANDROID_KEY_ALIAS"),
                "ANDROID_KEY_PASSWORD" to System.getenv("ANDROID_KEY_PASSWORD"),
            )
            mapOf(
                "storeFile" to needKeystore(env, "ANDROID_KEYSTORE_PATH", source),
                "storePassword" to need(env, "ANDROID_KEYSTORE_PASSWORD", source),
                "keyAlias" to need(env, "ANDROID_KEY_ALIAS", source),
                "keyPassword" to need(env, "ANDROID_KEY_PASSWORD", source),
            )
        } else if (propsFile.isFile) {
            val source = "файл ${propsFile.absolutePath}"
            val props = Properties()
            propsFile.inputStream().use { props.load(it) }
            val values = props.stringPropertyNames().associateWith { props.getProperty(it) }
            mapOf(
                "storeFile" to needKeystore(values, "storeFile", source),
                "storePassword" to need(values, "storePassword", source),
                "keyAlias" to need(values, "keyAlias", source),
                "keyPassword" to need(values, "keyPassword", source),
            )
        } else {
            null
        }
    }

    if (releaseSigning != null) {
        val keystoreFile = rootProject.file(releaseSigning.getValue("storeFile"))
        signingConfigs {
            create("release") {
                storeFile = keystoreFile
                storePassword = releaseSigning.getValue("storePassword")
                keyAlias = releaseSigning.getValue("keyAlias")
                keyPassword = releaseSigning.getValue("keyPassword")
            }
        }
    } else {
        logger.warn(
            "Plastinka: ключ подписи не задан, release будет неподписанным " +
                "(задайте ANDROID_KEYSTORE_PATH/_PASSWORD, ANDROID_KEY_ALIAS/_PASSWORD или android/keystore.properties)."
        )
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            if (releaseSigning != null) {
                signingConfig = signingConfigs.getByName("release")
            }
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
        compose = true
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.navigation.compose)

    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.graphics)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    debugImplementation(libs.compose.ui.tooling)

    implementation(libs.media3.exoplayer)
    implementation(libs.media3.session)
    implementation(libs.media3.okhttp)

    implementation(libs.okhttp)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.security.crypto)
    implementation(libs.coil.compose)

    testImplementation(libs.junit)
    testImplementation(libs.kotlin.test.junit)
}
