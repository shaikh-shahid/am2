#import <AppKit/AppKit.h>
#import <ScriptingBridge/ScriptingBridge.h>

// These selectors come from the installed Ghostty 1.3 AppleScript dictionary.
@interface AM2GhosttyApplication : SBApplication
- (SBElementArray *)windows;
@end

@interface AM2GhosttyWindow : SBObject
- (SBElementArray *)tabs;
- (NSString *)id;
@property (copy, readonly) NSString *name;
@end

@interface AM2GhosttyTab : SBObject
- (SBElementArray *)terminals;
@end

@interface AM2GhosttyTerminal : SBObject
- (NSString *)id;
@property (copy, readonly) NSString *name;
@property (copy, readonly) NSString *workingDirectory;
- (void)focus;
@end

static NSString *safeField(NSString *value) {
    if (value == nil) return @"";
    NSMutableString *result = [value mutableCopy];
    for (NSString *separator in @[@"\x1e", @"\x1f", @"\r", @"\n"]) {
        [result replaceOccurrencesOfString:separator withString:@" " options:0 range:NSMakeRange(0, result.length)];
    }
    return result;
}

static NSArray<NSRunningApplication *> *ghosttyInstances(void) {
    return [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.mitchellh.ghostty"];
}

static int listTerminals(void) {
    NSArray<NSRunningApplication *> *instances = ghosttyInstances();
    if (instances.count == 0) {
        fputs("No running Ghostty app instances found\n", stderr);
        return 1;
    }
    BOOL hadError = NO;
    for (NSRunningApplication *running in instances) {
        pid_t pid = running.processIdentifier;
        AM2GhosttyApplication *app = (AM2GhosttyApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        if (app == nil) {
            fprintf(stderr, "Ghostty instance %d has no scripting interface\n", pid);
            hadError = YES;
            continue;
        }
        for (AM2GhosttyWindow *window in [app windows]) {
            NSString *windowID = [window id];
            NSString *windowTitle = window.name;
            for (AM2GhosttyTab *tab in [window tabs]) {
                for (AM2GhosttyTerminal *terminal in [tab terminals]) {
                    NSArray<NSString *> *fields = @[
                        [NSString stringWithFormat:@"%d", pid],
                        safeField([terminal id]),
                        safeField(terminal.name),
                        safeField(terminal.workingDirectory),
                        safeField(windowID),
                        safeField(windowTitle)
                    ];
                    NSString *record = [[fields componentsJoinedByString:@"\x1f"] stringByAppendingString:@"\x1e"];
                    fputs(record.UTF8String, stdout);
                }
            }
        }
        NSError *error = [app lastError];
        if (error != nil) {
            fprintf(stderr, "Ghostty instance %d: %s\n", pid, error.localizedDescription.UTF8String);
            hadError = YES;
        }
    }
    return hadError ? 1 : 0;
}

static int focusTerminal(pid_t pid, NSString *terminalID) {
    BOOL foundInstance = NO;
    for (NSRunningApplication *running in ghosttyInstances()) {
        if (running.processIdentifier != pid) continue;
        foundInstance = YES;
        AM2GhosttyApplication *app = (AM2GhosttyApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        if (app == nil) break;
        for (AM2GhosttyWindow *window in [app windows]) {
            for (AM2GhosttyTab *tab in [window tabs]) {
                for (AM2GhosttyTerminal *terminal in [tab terminals]) {
                    if (![[terminal id] isEqualToString:terminalID]) continue;
                    [terminal focus];
                    NSError *error = [terminal lastError];
                    if (error != nil) {
                        fprintf(stderr, "%s\n", error.localizedDescription.UTF8String);
                        return 1;
                    }
                    return 0;
                }
            }
        }
    }
    fputs(foundInstance ? "Ghostty terminal is no longer open\n" : "Ghostty app instance is no longer running\n", stderr);
    return 1;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc == 2 && strcmp(argv[1], "list") == 0) return listTerminals();
        if (argc == 4 && strcmp(argv[1], "focus") == 0) {
            char *end = NULL;
            long pid = strtol(argv[2], &end, 10);
            if (pid <= 0 || end == argv[2] || *end != '\0') return 2;
            return focusTerminal((pid_t)pid, [NSString stringWithUTF8String:argv[3]]);
        }
        fputs("usage: ghostty-bridge list | focus PID TERMINAL_ID\n", stderr);
        return 2;
    }
}
