# Kubuno Mail — common

Reserved for the complete Kubuno Mail app, shared by Android and iOS: screens, view models, repositories and offline
store, written once in Kotlin Multiplatform with Compose Multiplatform. `../android` and `../ios` will then keep only
what each system does differently, and their entry points.

Empty for now: phase 1 moved the Android app here as it is (`../android/app`). The conversion plan (shared
libraries first, then the apps, Drive as the pilot) is in `core/mobile/README.md`, "Phase 2".
