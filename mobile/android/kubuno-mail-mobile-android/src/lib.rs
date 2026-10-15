//! Kubuno Mail for Android: the NativeActivity entry point. Everything Android does (window, GPU surface, font,
//! touch gestures, render loop) is the shared Kubuno Android shell's; this crate only hands it the app.

use android_activity::AndroidApp;

#[no_mangle]
fn android_main(app: AndroidApp) {
    kubuno_mobile_shell_android::run(app, kubuno_mail_mobile_common::APP);
}
