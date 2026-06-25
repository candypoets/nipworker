import org.gradle.api.tasks.compile.JavaCompile
import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.kotlin.gradle.tasks.KotlinCompile

plugins {
	id("com.android.library")
	id("org.jetbrains.kotlin.android")
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
	namespace = "com.candypoets.nipworker.lynx"
	compileSdk = providers.gradleProperty("ANDROID_COMPILE_SDK").map(String::toInt).orElse(35).get()

	defaultConfig {
		minSdk = providers.gradleProperty("ANDROID_MIN_SDK").map(String::toInt).orElse(23).get()
		consumerProguardFiles("consumer-rules.pro")
		ndk {
			abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86", "x86_64")
		}
	}

	compileOptions {
		sourceCompatibility = JavaVersion.VERSION_17
		targetCompatibility = JavaVersion.VERSION_17
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

tasks.withType<KotlinCompile>().configureEach {
	compilerOptions {
		jvmTarget.set(JvmTarget.JVM_17)
	}
}

val compileLynxStubs by tasks.registering(JavaCompile::class) {
	source = fileTree("src/compileOnlyStubs/java") {
		include("**/*.java")
	}
	classpath = files()
	destinationDirectory.set(layout.buildDirectory.dir("lynx-stubs/classes"))
	sourceCompatibility = JavaVersion.VERSION_17.toString()
	targetCompatibility = JavaVersion.VERSION_17.toString()
}

val lynxStubsJar by tasks.registering(Jar::class) {
	dependsOn(compileLynxStubs)
	archiveClassifier.set("lynx-compile-stubs")
	from(layout.buildDirectory.dir("lynx-stubs/classes"))
}

dependencies {
	compileOnly(files(lynxStubsJar))
	androidTestImplementation("androidx.test.ext:junit:1.2.1")

	// Optional override for host-specific Lynx/Sparkling SDK coordinates:
	// ./gradlew assembleRelease -PnipworkerLynxCompileOnly=com.example:lynx:1.0.0
	providers.gradleProperty("nipworkerLynxCompileOnly").orNull
		?.split(",")
		?.map(String::trim)
		?.filter(String::isNotEmpty)
		?.forEach { compileOnly(it) }
}

afterEvaluate {
	publishing {
		publications {
			create<MavenPublication>("release") {
				from(components["release"])
				artifactId = "nipworker-native-ffi-android"

				pom {
					name.set("NIPWorker Native FFI Android")
					description.set("Android AAR packaging the NIPWorker Rust native FFI and Lynx module")
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
