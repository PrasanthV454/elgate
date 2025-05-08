FROM rust:1.86-slim-bullseye

# Install required dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    cmake \
    git \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user to match your host user
ARG USER_ID=1000
ARG GROUP_ID=1000
RUN groupadd -g ${GROUP_ID} developer && \
    useradd -u ${USER_ID} -g ${GROUP_ID} -s /bin/bash -m developer

# Setup working directory
WORKDIR /code
RUN chown developer:developer /code

# Switch to the non-root user
USER developer

# Set environment variables
ENV RUST_BACKTRACE=1

# Use a default command that keeps the container running
CMD ["bash"] 