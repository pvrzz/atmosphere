/* atmosphere · pitch-worklet.js · Real-time pitch shifter (dual crossfaded delay taps) that changes pitch without changing tempo. */
class PitchShifter extends AudioWorkletProcessor {
  static get parameterDescriptors() {
    return [{ name: "ratio", defaultValue: 1, minValue: 0.25, maxValue: 4, automationRate: "k-rate" }];
  }

  constructor() {
    super();
    this.size = 1 << 16;
    this.mask = this.size - 1;
    this.buffers = [];
    this.write = 0;
    this.phase = 0;
    this.window = Math.round(sampleRate * 0.07);
  }

  tap(buf, pos) {
    const i = Math.floor(pos);
    const f = pos - i;
    const a = buf[i & this.mask];
    const b = buf[(i + 1) & this.mask];
    return a + (b - a) * f;
  }

  process(inputs, outputs, params) {
    const input = inputs[0];
    const output = outputs[0];
    if (!input || input.length === 0) return true;
    const channels = Math.min(input.length, output.length);
    while (this.buffers.length < channels) this.buffers.push(new Float32Array(this.size));
    const n = input[0].length;
    const ratio = params.ratio[0];
    const bypass = Math.abs(ratio - 1) < 0.001;
    const win = this.window;
    const step = (1 - ratio) / win;
    for (let i = 0; i < n; i++) {
      for (let c = 0; c < channels; c++) this.buffers[c][this.write] = input[c][i];
      if (bypass) {
        for (let c = 0; c < channels; c++) output[c][i] = input[c][i];
      } else {
        const p1 = this.phase;
        const p2 = (p1 + 0.5) % 1;
        const s1 = Math.sin(Math.PI * p1);
        const g1 = s1 * s1;
        const g2 = 1 - g1;
        const r1 = this.write - p1 * win;
        const r2 = this.write - p2 * win;
        for (let c = 0; c < channels; c++) {
          const buf = this.buffers[c];
          output[c][i] = this.tap(buf, r1) * g1 + this.tap(buf, r2) * g2;
        }
        this.phase += step;
        this.phase -= Math.floor(this.phase);
      }
      this.write = (this.write + 1) & this.mask;
    }
    return true;
  }
}

registerProcessor("pitch-shifter", PitchShifter);
