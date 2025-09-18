A good friend and brilliant engineer, Jordan Doyle, recently passed away unexpectedly in a terrible accident at only 25 years of age. In his thoughtful way, he had even set up a script meant to notify his family and provide instructions for such an event—but unfortunately, it never fired.

There were a few practical challenges: 
* The script was hosted on an external server funded by a bank account that froze as soon as the bank was notified of his passing. 
* His custom email address also depended on that account. 
* The system required zero activity for 45 days from any device he logged into to trigger, during which his family needed access to his devices to settle his affairs.
* Other potential issues, like whether failed logins counted as “check-ins,” added uncertainty.

This made me reflect on what a more robust architecture might look like—something
* That can run locally
* Requires only a configuration file
* supports multiple ways to check in
* reliably sends an emergency message with proper error handling.
* Check-ins shouldn’t be completely automatic; perhaps requiring a human response to an email or a heartbeat signal from a WHOOP like device.
* The system should be resilient to failures while remaining simple for those left behind.

In tribute to his love of Rust and Nix, I decided to learn Rust for the occasion—or at least enough to pair program with Claude Code! I also wrote an accompanying NixOS module [https://github.com/PulfordJ/nix-lastsignal-module]. His repositories can be found here: https://github.com/w4

README and example config: [https://github.com/PulfordJ/lastsignal]


