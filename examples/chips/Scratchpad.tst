// Uses the built-in RAM8 and reads its cells directly with RAM8[i].

load Scratchpad.hdl,
output-file Scratchpad.out,
compare-to Scratchpad.cmp,
output-list time%S1.4.1 in%D1.6.1 load%B2.1.2 address%D3.1.3 out%D1.6.1 twice%D1.6.1 RAM8[2]%D1.6.1;

set in 21, set load 1, set address 2,
tick, output;
tock, output;

set in 300, set address 5,
tick, output;
tock, output;

set load 0, set address 2,
eval, output;

set RAM8[2] -4,
eval, output;

set address 7,
tick, tock, output;
