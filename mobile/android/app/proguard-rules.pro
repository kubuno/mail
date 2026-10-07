# R8 keep-rules for the Kubuno mail app (release build).
#
# Most libraries ship their own consumer rules (Retrofit, OkHttp, Room, Hilt),
# but reflection-driven surfaces still need explicit keeps. Rules here are the
# audited minimum; keep them narrow so R8 can still shrink the rest.

# ---------------------------------------------------------------------------
# kotlinx.serialization
# The plugin generates a synthetic Companion + $$serializer per @Serializable
# class. R8 must keep those, and must not strip the @Serializable annotation it
# keys on. These rules cover the mail wire DTOs (com.kubuno.mail.net.*) and any
# serializable type shared from :core-account (com.kubuno.android.**).
# ---------------------------------------------------------------------------
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.**

# Keep the generated serializers referenced by name.
-if @kotlinx.serialization.Serializable class **
-keep, includedescriptorclasses class <1>$$serializer { *; }
-if @kotlinx.serialization.Serializable class **
-keepclassmembers class <1> {
    *** Companion;
}
-if @kotlinx.serialization.Serializable class ** {
    static **$* *;
}
-keepclassmembers class <2>$<3> {
    kotlinx.serialization.KSerializer serializer(...);
}

# Belt-and-braces for the concrete DTO packages this app (de)serializes.
-keep @kotlinx.serialization.Serializable class com.kubuno.mail.net.** { *; }
-keep @kotlinx.serialization.Serializable class com.kubuno.android.** { *; }

# ---------------------------------------------------------------------------
# Retrofit / OkHttp
# Retrofit reflects over the service interface (MailApi) and its generic
# signatures; keep them and silence platform-only warnings.
# ---------------------------------------------------------------------------
-keepattributes Signature, Exceptions, RuntimeVisibleAnnotations, RuntimeVisibleParameterAnnotations
-keep,allowobfuscation interface retrofit2.Call
-keep interface com.kubuno.mail.net.MailApi { *; }
-keepclasseswithmembers,allowobfuscation interface * {
    @retrofit2.http.* <methods>;
}
-dontwarn okhttp3.internal.**
-dontwarn okio.**
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**

# ---------------------------------------------------------------------------
# Room
# Room ships consumer rules; keep its generated *_Impl classes explicitly to be
# safe against aggressive shrinking.
# ---------------------------------------------------------------------------
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep @androidx.room.Entity class * { *; }
-dontwarn androidx.room.paging.**

# ---------------------------------------------------------------------------
# Hilt / Dagger — ships its own rules; this only silences generated-code notes.
# ---------------------------------------------------------------------------
-dontwarn dagger.hilt.**

# ---------------------------------------------------------------------------
# UnifiedPush connector
# The receiver is referenced from the manifest (kept automatically), but the
# Parcelable data types are created reflectively via their CREATOR fields.
# ---------------------------------------------------------------------------
-keep class org.unifiedpush.android.connector.data.** { *; }
-keepclassmembers class org.unifiedpush.android.connector.** {
    public static final ** CREATOR;
}
-dontwarn org.unifiedpush.android.connector.**
