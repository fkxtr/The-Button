/** Two quiet oscillators make an original, short mechanical-electronic click. */
export class ButtonAudio {
  private context?: AudioContext;

  async click(): Promise<void> {
    this.context ??= new AudioContext();
    if (this.context.state === "suspended") await this.context.resume();
    if (this.context.state !== "running") return;

    const context = this.context;
    const now = context.currentTime;
    const gain = context.createGain();
    gain.gain.setValueAtTime(0.0001, now);
    gain.gain.exponentialRampToValueAtTime(0.075, now + 0.004);
    gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.085);
    gain.connect(context.destination);

    for (const [frequency, endFrequency, type] of [
      [560, 95, "sine"],
      [1320, 440, "triangle"],
    ] as const) {
      const oscillator = context.createOscillator();
      oscillator.type = type;
      oscillator.frequency.setValueAtTime(frequency, now);
      oscillator.frequency.exponentialRampToValueAtTime(endFrequency, now + 0.065);
      oscillator.connect(gain);
      oscillator.start(now);
      oscillator.stop(now + 0.09);
      oscillator.onended = () => oscillator.disconnect();
    }
    window.setTimeout(() => gain.disconnect(), 150);
  }

  async close(): Promise<void> {
    if (this.context && this.context.state !== "closed") await this.context.close();
  }
}
