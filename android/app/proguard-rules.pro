# kotlinx.serialization: keep the generated serializers of the /api types (in :core).
-keepattributes *Annotation*, InnerClasses, Signature
-dontnote kotlinx.serialization.**
-keep,includedescriptorclasses class dev.cedar.core.**$$serializer { *; }
-keepclassmembers class dev.cedar.core.** {
    *** Companion;
}
-keepclasseswithmembers class dev.cedar.core.** {
    kotlinx.serialization.KSerializer serializer(...);
}

# OkHttp and its optional TLS providers.
-dontwarn okhttp3.internal.platform.**
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**

# Tink (UnifiedPush's Web Push keys) refers to classes it doesn't need on Android.
-dontwarn com.google.errorprone.annotations.**
-dontwarn javax.annotation.**
