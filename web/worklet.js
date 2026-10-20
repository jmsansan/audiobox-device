// The audio worklet of audiobox-device: the one piece of JavaScript in the crate.
//
// It runs on the browser's audio thread. It instantiates the WebAssembly module that the
// page compiled and passed in `processorOptions.module`, copies each message from the page
// into the module's memory and applies it, renders each 128-frame quantum through the
// module, and sends the module's reports back to the page. The mixer and all its logic are
// Rust, in the module.
class AudioboxDevice extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.exports = null;
    this.pending = [];
    this.port.onmessage = (event) => {
      this.pending.push(event.data);
      if (this.exports) this.applyPending();
    };
    WebAssembly.instantiate(options.processorOptions.module, {}).then((instance) => {
      this.exports = instance.exports;
      this.applyPending();
    });
  }

  applyPending() {
    const x = this.exports;
    for (const message of this.pending) {
      const bytes = new Uint8Array(message);
      const pointer = x.input(bytes.length);
      new Uint8Array(x.memory.buffer, pointer, bytes.length).set(bytes);
      x.apply(bytes.length);
    }
    this.pending.length = 0;
    this.sendReports();
  }

  sendReports() {
    const x = this.exports;
    const length = x.reports_len();
    if (length === 0) return;
    const copy = new Uint8Array(x.memory.buffer, x.reports_ptr(), length).slice();
    x.reports_clear();
    this.port.postMessage(copy.buffer, [copy.buffer]);
  }

  process(_inputs, outputs) {
    const x = this.exports;
    const output = outputs[0];
    if (!x || x.channels() === 0) return true;
    const frames = output[0].length;
    const channels = x.channels();
    const samples = new Float32Array(x.memory.buffer, x.render(frames), frames * channels);
    for (let channel = 0; channel < output.length; channel++) {
      const target = output[channel];
      const source = Math.min(channel, channels - 1);
      for (let i = 0; i < frames; i++) target[i] = samples[i * channels + source];
    }
    this.sendReports();
    return true;
  }
}

registerProcessor("audiobox-device", AudioboxDevice);
