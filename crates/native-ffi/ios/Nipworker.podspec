Pod::Spec.new do |s|
  s.name           = 'Nipworker'
  s.version        = '0.96.0'
  s.summary        = 'NIPWorker Lynx native module'
  s.description    = 'Rust Nostr engine exposed as a Lynx native module'
  s.license        = { :type => 'MIT' }
  s.author         = { 'Candy Poets' => 'dev@candypoets.com' }
  s.homepage       = 'https://github.com/candypoets/nipworker'
  s.platforms      = { :ios => '12.0' }
  s.source         = { :path => '.' }
  s.static_framework = true

  # Objective-C++ bridge source
  s.source_files   = 'LynxNipworkerModule.{h,mm}'
  s.public_header_files = 'LynxNipworkerModule.h'

  # Prebuilt Rust static library – must be built for iOS separately
  # Place the universal (or device/simulator) .a here:
  s.vendored_libraries = 'libnipworker_native_ffi.a'

  s.pod_target_xcconfig = {
    'OTHER_CPLUSPLUSFLAGS' => '$(inherited) -std=c++17',
    'LIBRARY_SEARCH_PATHS' => '$(inherited) "$(PODS_TARGET_SRCROOT)"',
  }

  s.dependency 'Lynx/Framework'
end
