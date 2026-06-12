FROM docker/sandbox-templates:claude-code-minimal

USER root
RUN apt update
RUN apt install -y build-essential clang libgtk-3-dev libjack-jackd2-dev

USER agent
SHELL ["/bin/bash", "-c"]
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# $BASH_ENV is sourced before non-interactive bash shells used by the agent's Bash tool calls
RUN echo ". \$HOME/.cargo/env" | tee -a "$BASH_ENV"

RUN rustup component add rust-analyzer rust-src

RUN git config --global user.name "Jim Posen"
RUN git config --global user.email "jim.posen@gmail.com"

