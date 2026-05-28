Pod::Spec.new do |s|
  s.name           = 'NipworkerReactNative'
  s.version        = '0.96.0'
  s.summary        = 'NIPWorker React Native native module'
  s.description    = 'Rust Nostr engine exposed as a React Native native module'
  s.homepage       = 'https://github.com/candypoets/nipworker'
  s.license        = { :type => 'MIT' }
  s.author         = { 'Candypoets' => 'sotachi@proton.me' }
  s.platforms      = { :ios => '13.0' }
  s.source         = { :git => 'https://github.com/candypoets/nipworker.git', :tag => "v#{s.version}" }
  s.source_files   = 'crates/native-ffi/react-native/ios/NipworkerReactNativeModule.{h,mm}'
  s.public_header_files = 'crates/native-ffi/react-native/ios/NipworkerReactNativeModule.h'
  s.vendored_frameworks = 'crates/native-ffi/ios/NipworkerNativeFFI.xcframework'
  s.dependency 'React-Core'
  s.dependency 'React-jsi'
  s.dependency 'React-cxxreact'
  s.pod_target_xcconfig = {
    'CLANG_CXX_LANGUAGE_STANDARD' => 'c++17',
    'CLANG_CXX_LIBRARY' => 'libc++',
    'OTHER_LDFLAGS' => '$(inherited) -lc++'
  }
end
