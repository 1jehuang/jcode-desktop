#import <AppKit/AppKit.h>

// The framework is loaded from this app's sealed bundle instead of being
// linked at build time. This keeps ordinary Rust builds independent of a local
// Sparkle installation while ensuring packaged builds never load a framework
// from an attacker-controlled search path.
static id updaterController;
static id updaterDelegate;

// Sparkle hands us a block that installs the staged update immediately. It is
// retained so the workspace's "restart" chip can run the install the user just
// asked for, rather than waiting for the next quit.
static void (^immediateInstall)(void);

// Mirrors `UpdateState` in crates/jcode-desktop-ui/src/updates.rs. The Rust
// side owns the user-facing wording; this file only reports which state we are
// in, so the two can never disagree about phrasing.
typedef NS_ENUM(uint32_t, JcodeUpdateState) {
    JcodeUpdateStateIdle = 0,
    JcodeUpdateStateChecking = 1,
    JcodeUpdateStateAvailable = 2,
    JcodeUpdateStateReady = 3,
};

extern void jcode_update_report(uint32_t state, const char *version);
extern void jcode_update_register_actions(void (*check_now)(void), void (*install_now)(void));

static void JcodeReport(JcodeUpdateState state, NSString *version)
{
    jcode_update_report((uint32_t)state, version.UTF8String);
}

// Called from Rust when the user clicks the update chip. Sparkle's install
// path must run on the main thread.
static void JcodeInstallNow(void)
{
    void (^install)(void) = immediateInstall;
    if (install == nil) {
        return;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        install();
    });
}

// A local declaration is sufficient for ARC and compile-time type checking.
// The actual class is still resolved only after the bundled framework loads.
@interface SPUStandardUpdaterController : NSObject
@property (nonatomic, readonly) id updater;
- (instancetype)initWithStartingUpdater:(BOOL)startUpdater
                        updaterDelegate:(id)updaterDelegate
                     userDriverDelegate:(id)userDriverDelegate;
@end

// Sparkle hands the delegate `SUAppcastItem`s. Only a version string is needed,
// so a minimal declaration keeps this file free of Sparkle headers.
@interface SUAppcastItem : NSObject
@property (nonatomic, readonly) NSString *displayVersionString;
@property (nonatomic, readonly) NSString *versionString;
@end

static NSString *JcodeVersionOf(id item)
{
    NSString *version = nil;
    if ([item respondsToSelector:@selector(displayVersionString)]) {
        version = ((SUAppcastItem *)item).displayVersionString;
    }
    if (version.length == 0 && [item respondsToSelector:@selector(versionString)]) {
        version = ((SUAppcastItem *)item).versionString;
    }
    return version ?: @"";
}

// The updater delegate. Sparkle calls these as a check progresses, and each one
// is forwarded to the Rust UI, so the workspace can show what is happening
// instead of updating silently in the background.
@interface JcodeUpdaterDelegate : NSObject
@end

@implementation JcodeUpdaterDelegate

- (void)updater:(id)updater didFinishLoadingAppcast:(id)appcast
{
    (void)updater;
    (void)appcast;
    JcodeReport(JcodeUpdateStateChecking, @"");
}

- (void)updaterDidNotFindUpdate:(id)updater
{
    (void)updater;
    JcodeReport(JcodeUpdateStateIdle, @"");
}

- (void)updater:(id)updater didFindValidUpdate:(id)item
{
    (void)updater;
    JcodeReport(JcodeUpdateStateAvailable, JcodeVersionOf(item));
}

- (void)updater:(id)updater didDownloadUpdate:(id)item
{
    (void)updater;
    JcodeReport(JcodeUpdateStateAvailable, JcodeVersionOf(item));
}

- (void)updater:(id)updater
      willInstallUpdateOnQuit:(id)item
    immediateInstallationBlock:(void (^)(void))immediateInstallHandler
{
    (void)updater;
    immediateInstall = [immediateInstallHandler copy];
    JcodeReport(JcodeUpdateStateReady, JcodeVersionOf(item));
}

- (void)updater:(id)updater didAbortWithError:(NSError *)error
{
    (void)updater;
    NSLog(@"Jcode update check failed: %@", error);
    JcodeReport(JcodeUpdateStateIdle, @"");
}

@end

@interface JcodeUpdaterBootstrap : NSObject
+ (void)checkInBackground;
@end

// Called from Rust for `/update`. Keep all Sparkle interaction on AppKit's main
// thread even when the command originates in a session callback.
static void JcodeCheckNow(void)
{
    dispatch_async(dispatch_get_main_queue(), ^{
        JcodeReport(JcodeUpdateStateChecking, @"");
        [JcodeUpdaterBootstrap checkInBackground];
    });
}

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

    updaterDelegate = [[JcodeUpdaterDelegate alloc] init];
    updaterController = [(SPUStandardUpdaterController *)[controllerClass alloc]
        initWithStartingUpdater:YES
                updaterDelegate:updaterDelegate
             userDriverDelegate:nil];

    jcode_update_register_actions(JcodeCheckNow, JcodeInstallNow);

    // A launch-time check turns "a fix shipped" into something the user sees
    // now, instead of up to a scheduled day later. It matters most for a build
    // broken badly enough that the user would otherwise just quit and give up.
    JcodeReport(JcodeUpdateStateChecking, @"");
    [JcodeUpdaterBootstrap checkInBackground];
}

+ (void)checkInBackground
{
    id updater = [updaterController respondsToSelector:@selector(updater)]
        ? ((SPUStandardUpdaterController *)updaterController).updater
        : nil;
    SEL check = @selector(checkForUpdatesInBackground);
    if (![updater respondsToSelector:check]) {
        JcodeReport(JcodeUpdateStateIdle, @"");
        return;
    }
    // A void, no-argument selector is safe to send this way, and it avoids
    // declaring the whole SPUUpdater interface here.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Warc-performSelector-leaks"
    [updater performSelector:check];
#pragma clang diagnostic pop
}

@end
