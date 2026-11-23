load Accumulator.hdl,
output-file Accumulator.out,
compare-to Accumulator.cmp,
output-list time%S1.4.1 in%D1.6.1 add%B2.1.2 reset%B2.1.2 out%D1.6.1;

set in 5, set add 1, set reset 0,
tick, output;
tock, output;

set in 7,
tick, output;
tock, output;

set add 0,
tick, output;
tock, output;

set in -20, set add 1,
repeat 3 {
    tick, tock, output;
}

set reset 1,
tick, output;
tock, output;

set reset 0, set in 32767,
tick, tock, output;
tick, tock, output;
