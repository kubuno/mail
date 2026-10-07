// Kubuno Mail — the mobile app of the Mail module.
//
// The Gradle root is mobile/; projects live in per-platform folders: android/
// today, common/ once the Kotlin Multiplatform conversion lands (README.md).
// The libraries shared by every Kubuno app (com.kubuno.mobile:*) come from the
// core repository (core/mobile), as published Maven artifacts.
pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

// Development against a core checkout (and the public CI): -Pkubuno.coreMobile=<path
// to core/mobile> or KUBUNO_CORE_MOBILE substitutes the com.kubuno.mobile:* artifacts
// with the projects of that build. Unset, the published versions are used.
val coreMobile: String? = providers.gradleProperty("kubuno.coreMobile")
    .orElse(providers.environmentVariable("KUBUNO_CORE_MOBILE"))
    .orNull?.takeIf { it.isNotBlank() }

// The Kubuno Maven registry (GitLab package registry of the kubuno group). It is
// private: the token comes from ~/.gradle/gradle.properties (kubunoGitlabToken) or
// KUBUNO_GITLAB_TOKEN, never from this repository.
val kubunoMavenUrl: String = providers.gradleProperty("kubunoMavenUrl")
    .orElse("http://gitlab.olinga.lan/api/v4/groups/kubuno/-/packages/maven").get()
val kubunoToken: String? = providers.gradleProperty("kubunoGitlabToken")
    .orElse(providers.environmentVariable("KUBUNO_GITLAB_TOKEN")).orNull
val kubunoTokenHeader: String = providers.gradleProperty("kubunoGitlabTokenHeader")
    .orElse("Private-Token").get()

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        if (coreMobile == null) {
            maven {
                name = "kubuno"
                url = uri(kubunoMavenUrl)
                if (url.scheme == "http" || url.scheme == "https") {
                    isAllowInsecureProtocol = url.scheme == "http"
                    if (kubunoToken != null) {
                        credentials(HttpHeaderCredentials::class) {
                            name = kubunoTokenHeader
                            value = kubunoToken
                        }
                        authentication { create<HttpHeaderAuthentication>("header") }
                    }
                }
                content { includeGroup("com.kubuno.mobile") }
            }
        }
    }
}

if (coreMobile != null) {
    includeBuild(coreMobile)
}

rootProject.name = "kubuno-mail-mobile"

include(":app")
project(":app").projectDir = file("android/app")
