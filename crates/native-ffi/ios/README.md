# NIPWorker iOS Native FFI

Run:

```bash
./build-ios.sh
```

This builds `NipworkerNativeFFI.xcframework` for iOS device, iOS simulator, and
macOS. It includes `nipworker.h` and a Clang module map. React Native iOS and
the Swift package both link this static XCFramework; it should not be embedded
as a dynamic framework.
