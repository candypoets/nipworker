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
  s.platforms      = { :ios => '13.0' }
  s.source         = { :git => 'https://github.com/candypoets/nipworker.git', :tag => "v#{s.version}" }
  s.source_files   = 'NipworkerReactNativeModule.{h,mm}'
  s.public_header_files = 'NipworkerReactNativeModule.h'
  s.vendored_frameworks = '../../ios/NipworkerNativeFFI.xcframework'
  s.dependency 'React-Core'
  s.dependency 'React-jsi'
  s.dependency 'React-cxxreact'
  s.pod_target_xcconfig = {
    'CLANG_CXX_LANGUAGE_STANDARD' => 'c++17',
    'CLANG_CXX_LIBRARY' => 'libc++',
    'OTHER_LDFLAGS' => '$(inherited) -lc++'
  }
end
