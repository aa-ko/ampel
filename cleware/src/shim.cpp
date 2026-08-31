/* extern "C" bridge to the vendor driver. This file exists for two reasons:
 *   - doSwitch() is C++, so its symbol is mangled and unreachable from Rust;
 *   - the switchID convention it expects (SWITCH_0 first, a 0 terminates the
 *     list) only lived inside the vendor main(), which the build renames away.
 */
#include <unistd.h> // USBaccess.h calls usleep() without including it itself
#include "USBaccess.h"

const int MAXSWITCH = 16; // MAXSWITCH from USBswitchCmd.cpp, not exported by a header

extern int doSwitch(int debug, int doRead, int printRead, int secureSwitching,
                    int turnSwitch, int serialNumber, int traffic, int binaryMode,
                    int resetDevice, int intervalTestMS, int intervalCount,
                    char **pulsieren, int switchID[]);

/* Drive the three lamps from a tl_enum bitmask. Returns 0 on success, -1 if no
 * device answered. doSwitch() opens and closes the device itself, so repeated
 * calls need no state on our side. */
extern "C" int cleware_set_traffic(int traffic) {
    int switchID[MAXSWITCH] = {CUSBaccess::SWITCH_0};
    /* turnSwitch must be 0: with a traffic mask set doSwitch drives the lamps
     * directly, while any other value is taken as a read command. */
    return doSwitch(0, 0, 0, 0, 0, 0, traffic, 0, 0, 0, 0, 0, switchID);
}
