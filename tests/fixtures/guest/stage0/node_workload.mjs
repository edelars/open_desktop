const count = 5_000_000;
const seed = 0x6d2b79f5;
let state = seed;
let digest = 0;

for (let index = 0; index < count; index += 1) {
  state = (Math.imul(state, 1_664_525) + 1_013_904_223) >>> 0;
  digest = (Math.imul(digest ^ state, 2_246_822_519) + index) >>> 0;
}

console.log(JSON.stringify({ count, digest, seed }));
