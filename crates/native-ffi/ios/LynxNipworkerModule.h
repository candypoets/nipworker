//
//  LynxNipworkerModule.h
//  NIPWorker Lynx native module
//

#import <Foundation/Foundation.h>
#import <Lynx/LynxContextModule.h>

@interface NipworkerLynxModule : NSObject <LynxContextModule>

- (instancetype)initWithLynxContext:(LynxContext *)context;

@end
