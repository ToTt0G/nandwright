// A script that waits for the keyboard, like the official Memory test.
// Nandwright presses the awaited key for you.

load KeyLatch.hdl,
output-file KeyLatch.out,
compare-to KeyLatch.cmp,
output-list store%B2.1.2 key%D1.6.1 last%D1.6.1;

set store 0, eval, output;

echo "Hold down the 'K' key until you see the next message",
while key <> 75 {
    eval,
}
clear-echo,
set store 1, tick, tock, output;

set store 0, tick, tock, output;
