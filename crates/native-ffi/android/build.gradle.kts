plugins {
	id("com.android.library")
	id("maven-publish")
}

group = "com.candypoets"
version = providers.gradleProperty("VERSION_NAME").orElse(nativeFfiVersion()).get()
base.archivesName.set("nipworker-native-ffi-android")

fun nativeFfiVersion(): String {
	val cargoToml = layout.projectDirectory.file("../Cargo.toml").asFile
	if (cargoToml.isFile) {
		val match = Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(cargoToml.readText())
		if (match != null) {
			return match.groupValues[1]
		}
	}

	val packageJson = layout.projectDirectory.file("../../../package.json").asFile
	if (packageJson.isFile) {
		val match = Regex(""""version"\s*:\s*"([^"]+)"""").find(packageJson.readText())
		if (match != null) {
			return match.groupValues[1]
		}
	}

	return "0.0.0"
}

android {
	namespace = "com.candypoets.nipworker.nativeffi"
	compileSdk = providers.gradleProperty("ANDROID_COMPILE_SDK").map(String::toInt).orElse(35).get()

	defaultConfig {
		minSdk = providers.gradleProperty("ANDROID_MIN_SDK").map(String::toInt).orElse(23).get()
		ndk {
			abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86", "x86_64")
		}
	}

	sourceSets {
		getByName("main") {
			jniLibs.srcDir("src/main/jniLibs")
		}
	}

	publishing {
		singleVariant("release") {
			withSourcesJar()
		}
	}
}

dependencies {
	androidTestImplementation("androidx.test.ext:junit:1.2.1")
}

afterEvaluate {
	publishing {
		publications {
			create<MavenPublication>("release") {
				from(components["release"])
				artifactId = "nipworker-native-ffi-android"

					pom {
						name.set("NIPWorker Native FFI Android")
						description.set("Android AAR packaging the NIPWorker Rust native FFI libraries")
					url.set("https://github.com/candypoets/nipworker")
					licenses {
						license {
							name.set("MIT")
						}
					}
					developers {
						developer {
							name.set("Candy Poets")
							email.set("dev@candypoets.com")
						}
					}
					scm {
						url.set("https://github.com/candypoets/nipworker")
					}
				}
			}
		}
	}
}
