//! Kubuno Mail for iOS: the app's entry point. Everything iOS does (UIWindow, Metal surface, font, safe area,
//! render loop) is the shared Kubuno iOS shell's; this crate only hands it the app.

fn main() {
    kubuno_mobile_shell_ios::run(kubuno_mail_mobile_common::APP);
}
