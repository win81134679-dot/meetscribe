// AGP 9 內建 Kotlin 編譯，不用再套 org.jetbrains.kotlin.android；Compose 編譯器外掛仍要另外套。
plugins {
    id("com.android.application") version "9.5.0" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
}
