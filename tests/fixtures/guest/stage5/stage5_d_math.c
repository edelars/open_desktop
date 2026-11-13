unsigned stage5_sum(void) {
    unsigned sum = 0;
    for (unsigned value = 1; value <= 100; ++value) {
        sum += value;
    }
    return sum;
}
