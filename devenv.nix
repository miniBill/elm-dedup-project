{ pkgs, ... }:

{
  packages = with pkgs; [
    git
    pkg-config
    openssl
  ];
  languages.rust.enable = true;
  # processes.ping.exec = "ping example.com";
}
