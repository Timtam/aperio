#import "AperioReactHost.h"

#import <Expo/RCTAppDelegateUmbrella.h>
#import <React/RCTDevMenu.h>

BOOL AperioHasReactHost(id factory)
{
  if (![factory isKindOfClass:[RCTReactNativeFactory class]]) {
    return NO;
  }
  return ((RCTReactNativeFactory *)factory).rootViewFactory.reactHost != nil;
}

void AperioStartReactHost(id factory)
{
  if (![factory isKindOfClass:[RCTReactNativeFactory class]]) {
    return;
  }
  RCTReactNativeFactory *reactNativeFactory = (RCTReactNativeFactory *)factory;
  // What a scene's start passes: the factory's bundle (Expo's override names
  // the app's embedded bundle in a release build) and its dev menu
  // configuration, or the default one where a release build has none.
  RCTDevMenuConfiguration *devMenuConfiguration =
      reactNativeFactory.devMenuConfiguration ?: [RCTDevMenuConfiguration defaultConfiguration];
  [reactNativeFactory.rootViewFactory initializeReactHostWithLaunchOptions:nil
                                                       bundleConfiguration:reactNativeFactory.bundleConfiguration
                                                      devMenuConfiguration:devMenuConfiguration];
}
