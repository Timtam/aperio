#import <ExpoModulesCore/EXTaskServiceInterface.h>

NS_ASSUME_NONNULL_BEGIN

/// Reaches Expo's task service without linking against it.
///
/// `BackgroundRefresh.swift` needs to run the app's registered TaskManager
/// tasks from its own `BGAppRefreshTask` handler, and that means calling
/// `EXTaskService`. It lives in the ExpoTaskManager pod, which this module does
/// not depend on and should not have to: the lookup is by NAME at runtime, so a
/// build without the task manager simply gets nil and the refresh quietly does
/// nothing rather than failing to link.
///
/// Objective-C rather than Swift on purpose. Swift's `perform(_:)` tops out at
/// two arguments and cannot pass the completion block, and the protocol-typed
/// return is what makes the call site readable. This is the same trick Expo's
/// own `EXTaskServiceHelper` uses, under a different name so the two cannot
/// collide.
@interface AperioTaskServiceHelper : NSObject
+ (nullable id<EXTaskServiceInterface>)sharedTaskService;
@end

NS_ASSUME_NONNULL_END
