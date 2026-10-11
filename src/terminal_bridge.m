#import <AppKit/AppKit.h>
#import <ScriptingBridge/ScriptingBridge.h>

// Minimal interfaces generated from Terminal.app and iTerm2 scripting dictionaries.
@interface AM2TerminalApplication : SBApplication
- (SBElementArray *)windows;
@end
@interface AM2TerminalWindow : SBObject
- (SBElementArray *)tabs;
- (NSInteger)id;
@property (copy, readonly) NSString *name;
@property (copy) id selectedTab;
@property BOOL frontmost;
@end
@interface AM2TerminalTab : SBObject
@property (copy, readonly) NSString *tty;
@property (copy, readonly) NSString *contents;
@end

@interface AM2ITermApplication : SBApplication
- (SBElementArray *)windows;
@end
@interface AM2ITermWindow : SBObject
- (SBElementArray *)tabs;
- (NSInteger)id;
@property (copy, readonly) NSString *name;
- (void)select;
@end
@interface AM2ITermTab : SBObject
- (SBElementArray *)sessions;
@end
@interface AM2ITermSession : SBObject
- (NSString *)id;
@property (copy, readonly) NSString *tty;
@property (copy, readonly) NSString *text;
- (void)select;
@end

static NSString *safeField(NSString *value) {
    if (value == nil) return @"";
    NSMutableString *result = [value mutableCopy];
    for (NSString *separator in @[@"\x1e", @"\x1f", @"\r", @"\n"]) {
        [result replaceOccurrencesOfString:separator withString:@" " options:0 range:NSMakeRange(0, result.length)];
    }
    return result;
}

static void emit(NSString *provider, pid_t pid, NSString *identifier, NSString *tty,
                 NSString *title, NSString *windowID, NSString *windowTitle) {
    NSArray<NSString *> *fields = @[
        provider, [NSString stringWithFormat:@"%d", pid], safeField(identifier),
        safeField(tty), safeField(title), safeField(windowID), safeField(windowTitle)
    ];
    NSString *record = [[fields componentsJoinedByString:@"\x1f"] stringByAppendingString:@"\x1e"];
    fputs(record.UTF8String, stdout);
}

static int listTerminal(void) {
    BOOL hadError = NO;
    for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.Terminal"]) {
        pid_t pid = running.processIdentifier;
        AM2TerminalApplication *app = (AM2TerminalApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        if (app == nil) { fprintf(stderr, "Terminal.app instance %d has no scripting interface\n", pid); hadError = YES; continue; }
        for (AM2TerminalWindow *window in [app windows]) {
            NSString *windowID = [NSString stringWithFormat:@"%ld", (long)[window id]];
            for (AM2TerminalTab *tab in [window tabs]) {
                NSString *tty = tab.tty;
                if (tty.length == 0) continue;
                emit(@"terminal", pid, tty, tty, window.name, windowID, window.name);
            }
        }
        NSError *error = [app lastError];
        if (error != nil) { fprintf(stderr, "Terminal.app instance %d: %s\n", pid, error.localizedDescription.UTF8String); hadError = YES; }
    }
    return hadError ? 1 : 0;
}

static int listITerm(void) {
    BOOL hadError = NO;
    for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.googlecode.iterm2"]) {
        pid_t pid = running.processIdentifier;
        AM2ITermApplication *app = (AM2ITermApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        if (app == nil) { fprintf(stderr, "iTerm2 instance %d has no scripting interface\n", pid); hadError = YES; continue; }
        for (AM2ITermWindow *window in [app windows]) {
            NSString *windowID = [NSString stringWithFormat:@"%ld", (long)[window id]];
            for (AM2ITermTab *tab in [window tabs]) {
                for (AM2ITermSession *session in [tab sessions]) {
                    NSString *sessionID = [session id];
                    if (sessionID.length == 0) continue;
                    emit(@"iterm2", pid, sessionID, session.tty, window.name, windowID, window.name);
                }
            }
        }
        NSError *error = [app lastError];
        if (error != nil) { fprintf(stderr, "iTerm2 instance %d: %s\n", pid, error.localizedDescription.UTF8String); hadError = YES; }
    }
    return hadError ? 1 : 0;
}

static int activateInstance(NSRunningApplication *running) {
    // This helper is launched by a terminal TUI, so there is no NSApplication
    // instance that can yield activation cooperatively. The user explicitly
    // requested this focus change with the o key.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    BOOL accepted = [running activateWithOptions:NSApplicationActivateIgnoringOtherApps];
#pragma clang diagnostic pop
    if (!accepted) {
        fprintf(stderr, "macOS rejected activation of terminal app process %d\n", running.processIdentifier);
        return 1;
    }
    for (int attempt = 0; attempt < 20; attempt++) {
        if (running.isActive) return 0;
        // NSRunningApplication refreshes time-varying properties on the main
        // run loop. Sleeping here leaves isActive stuck at its old value.
        [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.05]];
    }
    fprintf(stderr, "Terminal app process %d did not become frontmost\n", running.processIdentifier);
    return 1;
}

static int focusTerminal(pid_t pid, NSString *tty) {
    for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.Terminal"]) {
        if (running.processIdentifier != pid) continue;
        AM2TerminalApplication *app = (AM2TerminalApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        for (AM2TerminalWindow *window in [app windows]) {
            for (AM2TerminalTab *tab in [window tabs]) {
                if (![tab.tty isEqualToString:tty]) continue;
                window.selectedTab = tab;
                window.frontmost = YES;
                [app activate];
                NSError *error = [app lastError];
                if (error != nil) { fprintf(stderr, "%s\n", error.localizedDescription.UTF8String); return 1; }
                return activateInstance(running);
            }
        }
        fputs("Terminal.app tab is no longer open\n", stderr);
        return 1;
    }
    fputs("Terminal.app instance is no longer running\n", stderr);
    return 1;
}

static int focusITerm(pid_t pid, NSString *sessionID) {
    for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.googlecode.iterm2"]) {
        if (running.processIdentifier != pid) continue;
        AM2ITermApplication *app = (AM2ITermApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        for (AM2ITermWindow *window in [app windows]) {
            for (AM2ITermTab *tab in [window tabs]) {
                for (AM2ITermSession *session in [tab sessions]) {
                    if (![[session id] isEqualToString:sessionID]) continue;
                    [session select];
                    [window select];
                    [app activate];
                    NSError *error = [app lastError];
                    if (error != nil) { fprintf(stderr, "%s\n", error.localizedDescription.UTF8String); return 1; }
                    return activateInstance(running);
                }
            }
        }
        fputs("iTerm2 session is no longer open\n", stderr);
        return 1;
    }
    fputs("iTerm2 instance is no longer running\n", stderr);
    return 1;
}

static int emitVisibleText(NSString *contents) {
    if (contents == nil) {
        fputs("Terminal did not return visible text\n", stderr);
        return 1;
    }
    NSData *data = [contents dataUsingEncoding:NSUTF8StringEncoding];
    if (data == nil) {
        fputs("Terminal text could not be encoded as UTF-8\n", stderr);
        return 1;
    }
    const NSUInteger cap = 16 * 1024;
    NSUInteger offset = data.length > cap ? data.length - cap : 0;
    const unsigned char *bytes = data.bytes;
    while (offset < data.length && (bytes[offset] & 0xc0) == 0x80) offset++;
    if (offset < data.length) fwrite(bytes + offset, 1, data.length - offset, stdout);
    return 0;
}

static int readTerminal(pid_t pid, NSString *tty) {
    for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.Terminal"]) {
        if (running.processIdentifier != pid) continue;
        AM2TerminalApplication *app = (AM2TerminalApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        for (AM2TerminalWindow *window in [app windows]) {
            for (AM2TerminalTab *tab in [window tabs]) {
                if (![tab.tty isEqualToString:tty]) continue;
                NSString *contents = tab.contents;
                NSError *error = [tab lastError];
                if (error != nil) { fprintf(stderr, "%s\n", error.localizedDescription.UTF8String); return 1; }
                return emitVisibleText(contents);
            }
        }
        fputs("Terminal.app tab is no longer open\n", stderr);
        return 1;
    }
    fputs("Terminal.app instance is no longer running\n", stderr);
    return 1;
}

static int readITerm(pid_t pid, NSString *sessionID) {
    for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.googlecode.iterm2"]) {
        if (running.processIdentifier != pid) continue;
        AM2ITermApplication *app = (AM2ITermApplication *)[SBApplication applicationWithProcessIdentifier:pid];
        for (AM2ITermWindow *window in [app windows]) {
            for (AM2ITermTab *tab in [window tabs]) {
                for (AM2ITermSession *session in [tab sessions]) {
                    if (![[session id] isEqualToString:sessionID]) continue;
                    NSString *contents = session.text;
                    NSError *error = [session lastError];
                    if (error != nil) { fprintf(stderr, "%s\n", error.localizedDescription.UTF8String); return 1; }
                    return emitVisibleText(contents);
                }
            }
        }
        fputs("iTerm2 session is no longer open\n", stderr);
        return 1;
    }
    fputs("iTerm2 instance is no longer running\n", stderr);
    return 1;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc == 2 && strcmp(argv[1], "list") == 0) {
            int terminal = listTerminal();
            int iterm = listITerm();
            return (terminal || iterm) ? 1 : 0;
        }
        if (argc == 5 && (strcmp(argv[1], "focus") == 0 || strcmp(argv[1], "read") == 0)) {
            char *end = NULL;
            long pid = strtol(argv[3], &end, 10);
            if (pid <= 0 || end == argv[3] || *end != '\0') return 2;
            NSString *identifier = [NSString stringWithUTF8String:argv[4]];
            BOOL focus = strcmp(argv[1], "focus") == 0;
            if (strcmp(argv[2], "terminal") == 0) return focus ? focusTerminal((pid_t)pid, identifier) : readTerminal((pid_t)pid, identifier);
            if (strcmp(argv[2], "iterm2") == 0) return focus ? focusITerm((pid_t)pid, identifier) : readITerm((pid_t)pid, identifier);
        }
        fputs("usage: terminal-bridge list | focus|read terminal|iterm2 PID ID\n", stderr);
        return 2;
    }
}
