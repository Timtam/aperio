#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

// The two calls into React Native's root view factory that Swift cannot make
// itself: its `reactHost` is declared with a forward-declared class, and the
// dev menu configuration it wants is nil in a release build while the
// parameter is nonnull. `factory` is the app's `RCTReactNativeFactory`; it is
// typed `id` so this header needs no React headers of its own.

/// Whether the factory's React host exists, that is, whether React Native was
/// started already (by a scene, or by an earlier call below).
FOUNDATION_EXPORT BOOL AperioHasReactHost(id factory);

/// Start the factory's React host without a surface: the JavaScript bundle
/// runs, no view is mounted. A scene that connects later finds the running
/// host and only adds its surface to it.
FOUNDATION_EXPORT void AperioStartReactHost(id factory);

NS_ASSUME_NONNULL_END
