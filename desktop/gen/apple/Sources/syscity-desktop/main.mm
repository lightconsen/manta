#import <Foundation/Foundation.h>
#include "bindings/bindings.h"

int main(int argc, char * argv[]) {
  // Establish SYSCITY_HOME under the app sandbox before the Rust gateway
  // starts (the gateway resolves data paths from this env var at startup).
  // Mirrors MainActivity.kt setting SYSCITY_HOME on Android.
  @autoreleasepool {
    NSString *docs = NSSearchPathForDirectoriesInDomains(NSDocumentDirectory, NSUserDomainMask, YES).firstObject;
    NSString *home = [docs stringByAppendingPathComponent:@"syscity"];
    [[NSFileManager defaultManager] createDirectoryAtPath:home withIntermediateDirectories:YES attributes:nil error:nil];
    setenv("SYSCITY_HOME", home.UTF8String, 1);
  }
  ffi::start_app();
  return 0;
}
