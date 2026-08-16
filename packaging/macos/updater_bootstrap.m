#import <AppKit/AppKit.h>

// The framework is loaded from this app's sealed bundle instead of being
// linked at build time. This keeps ordinary Rust builds independent of a local
// Sparkle installation while ensuring packaged builds never load a framework
// from an attacker-controlled search path.
static id updaterController;

// A local declaration is sufficient for ARC and compile-time type checking.
// The actual class is still resolved only after the bundled framework loads.
@interface SPUStandardUpdaterController : NSObject
- (instancetype)initWithStartingUpdater:(BOOL)startUpdater
                        updaterDelegate:(id)updaterDelegate
                     userDriverDelegate:(id)userDriverDelegate;
@end

@interface JcodeUpdaterBootstrap : NSObject
@end

@implementation JcodeUpdaterBootstrap

+ (void)load
{
    @autoreleasepool {
        [[NSNotificationCenter defaultCenter]
            addObserver:self
               selector:@selector(applicationDidFinishLaunching:)
                   name:NSApplicationDidFinishLaunchingNotification
                 object:nil];
    }
}

+ (void)applicationDidFinishLaunching:(NSNotification *)notification
{
    (void)notification;
    [[NSNotificationCenter defaultCenter] removeObserver:self];

    NSBundle *applicationBundle = [NSBundle mainBundle];
    NSDictionary *info = applicationBundle.infoDictionary;
    NSString *publicKey = info[@"SUPublicEDKey"];
    NSString *feedURL = info[@"SUFeedURL"];

    // Source and ad-hoc local packages intentionally omit update credentials.
    // A tagged release is prevented from reaching this state by CI.
    if (publicKey.length == 0 || feedURL.length == 0) {
        return;
    }

    NSURL *frameworkURL = [applicationBundle.privateFrameworksURL
        URLByAppendingPathComponent:@"Sparkle.framework"
                     isDirectory:YES];
    NSBundle *sparkleBundle = [NSBundle bundleWithURL:frameworkURL];
    NSError *loadError = nil;
    if (sparkleBundle == nil || ![sparkleBundle loadAndReturnError:&loadError]) {
        NSLog(@"Jcode automatic updates are unavailable: %@", loadError);
        return;
    }

    Class controllerClass = NSClassFromString(@"SPUStandardUpdaterController");
    SEL initializer = @selector(initWithStartingUpdater:updaterDelegate:userDriverDelegate:);
    if (controllerClass == Nil || ![controllerClass instancesRespondToSelector:initializer]) {
        NSLog(@"Jcode automatic updates are unavailable: incompatible Sparkle framework");
        return;
    }

    updaterController = [(SPUStandardUpdaterController *)[controllerClass alloc]
        initWithStartingUpdater:YES
               updaterDelegate:nil
            userDriverDelegate:nil];
}

@end
