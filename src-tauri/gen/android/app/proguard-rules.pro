# R8 runs in release builds (obfuscation and optimization on). The rules below keep what is reached by
# reflection or JNI: Tauri's plugin classes and their annotated commands and argument classes, and the
# barcode scanner stack (CameraX + ML Kit) behind QR pairing (SPEC §9.3).
-keepattributes *Annotation*,Signature,InnerClasses,EnclosingMethod,RuntimeVisibleAnnotations
-keep class kotlin.Metadata { *; }
-keep class app.tauri.** { *; }
-keep class com.alstroemeria.app.** { *; }
-keepclassmembers class * {
    @app.tauri.annotation.Command *;
    @app.tauri.annotation.PermissionCallback *;
    @app.tauri.annotation.ActivityCallback *;
}
-keep class com.google.mlkit.** { *; }
-keep class com.google.android.gms.** { *; }
-keep class androidx.camera.** { *; }
-dontwarn **
