require 'json'

package = JSON.parse(File.read(File.expand_path('../../../../package.json', __dir__)))

Pod::Spec.new do |s|
  s.name           = 'NipworkerReactNative'
  s.version        = package['version']
  s.summary        = 'NIPWorker React Native native module'
  s.description    = 'Rust Nostr engine exposed as a React Native native module'
  s.homepage       = 'https://github.com/candypoets/nipworker'
  s.license        = { :type => 'MIT' }
  s.author         = { 'Candypoets' => 'sotachi@proton.me' }
  s.platforms      = { :ios => '14.0' }
  s.source         = { :git => 'https://github.com/candypoets/nipworker.git', :tag => "v#{s.version}" }
  s.source_files   = [
    'NipworkerReactNativeModule.{h,mm}',
    '../../../../swift/Sources/NipworkerSwift/**/*.swift'
  ]
  s.public_header_files = 'NipworkerReactNativeModule.h'
  # This pod is the sole native-binary owner in a React Native application.
  # Its Objective-C++ and Swift facades therefore call into the same Rust image.
  s.vendored_frameworks = '../../ios/NipworkerNativeFFI.xcframework'
  s.dependency 'FlatBuffers', '~> 25.2.10'
  s.dependency 'React-Core'
  s.dependency 'React-jsi'
  s.dependency 'React-cxxreact'
  s.pod_target_xcconfig = {
    'CLANG_CXX_LANGUAGE_STANDARD' => 'c++17',
    'CLANG_CXX_LIBRARY' => 'libc++',
    'OTHER_LDFLAGS' => '$(inherited) -lc++'
  }
  s.swift_version = '5.9'
end
