// Tests every input combination of the Veto chip.

load Veto.hdl,
output-file Veto.out,
compare-to Veto.cmp,
output-list a%B3.1.3 b%B3.1.3 c%B3.1.3 veto%B3.1.3 most%B3.1.3 pass%B3.1.3;

set a 0, set b 0, set c 0, set veto 0, eval, output;
set a 0, set b 0, set c 0, set veto 1, eval, output;
set a 0, set b 0, set c 1, set veto 0, eval, output;
set a 0, set b 0, set c 1, set veto 1, eval, output;
set a 0, set b 1, set c 0, set veto 0, eval, output;
set a 0, set b 1, set c 0, set veto 1, eval, output;
set a 0, set b 1, set c 1, set veto 0, eval, output;
set a 0, set b 1, set c 1, set veto 1, eval, output;
set a 1, set b 0, set c 0, set veto 0, eval, output;
set a 1, set b 0, set c 0, set veto 1, eval, output;
set a 1, set b 0, set c 1, set veto 0, eval, output;
set a 1, set b 0, set c 1, set veto 1, eval, output;
set a 1, set b 1, set c 0, set veto 0, eval, output;
set a 1, set b 1, set c 0, set veto 1, eval, output;
set a 1, set b 1, set c 1, set veto 0, eval, output;
set a 1, set b 1, set c 1, set veto 1, eval, output;
