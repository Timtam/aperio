#import "AperioTaskServiceHelper.h"

/// What `EXTaskService` answers to, declared here so `@selector(shared)` names
/// a known selector without importing the ExpoTaskManager pod.
@protocol AperioTaskServiceSingleton <NSObject>
+ (id)shared;
@end

@implementation AperioTaskServiceHelper

+ (nullable id<EXTaskServiceInterface>)sharedTaskService {
  Class taskServiceClass = NSClassFromString(@"EXTaskService");
  if (![taskServiceClass respondsToSelector:@selector(shared)]) {
    return nil;
  }
  id instance = [taskServiceClass performSelector:@selector(shared)];
  if (![instance conformsToProtocol:@protocol(EXTaskServiceInterface)]) {
    return nil;
  }
  return (id<EXTaskServiceInterface>)instance;
}

@end
