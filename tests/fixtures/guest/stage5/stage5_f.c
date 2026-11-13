#define _POSIX_C_SOURCE 200112L
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <signal.h>
#include <stdint.h>
#include <sys/select.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

enum {
    CONNECT_NETWORK_FAILED = -1,
    CONNECT_NAME_OR_SERVICE_ERROR = -2,
    CONNECT_LOCAL_ADDRESS_REFUSED = -3,
    CONNECT_SOCKET_SETUP_ERROR = -4,
};

static int is_network_connect_error(int error) {
    switch (error) {
    case ECONNREFUSED:
    case ETIMEDOUT:
    case ENETUNREACH:
    case EHOSTUNREACH:
        return 1;
    default:
        return 0;
    }
}

static int is_expected_drop_error(int error) {
    return error == ECONNRESET || error == EPIPE;
}

static int is_expected_drop_response(ssize_t received, int error) {
    return received == 0 ||
           (received < 0 && is_expected_drop_error(error));
}

static int classify_connect_outcome(int setup_error, int connect_attempted,
                                    int rejected_local_address) {
    if (setup_error) {
        return CONNECT_SOCKET_SETUP_ERROR;
    }
    if (connect_attempted) {
        return CONNECT_NETWORK_FAILED;
    }
    if (rejected_local_address) {
        return CONNECT_LOCAL_ADDRESS_REFUSED;
    }
    return CONNECT_SOCKET_SETUP_ERROR;
}

static int valid_remote_host(const char *host) {
    return strcmp(host, "localhost") != 0 && strncmp(host, "127.", 4) != 0 &&
           strcmp(host, "::1") != 0 && strcmp(host, "0.0.0.0") != 0 &&
           strcmp(host, "::") != 0;
}

static int address_is_remote(const struct addrinfo *address) {
    if (address->ai_family == AF_INET) {
        const struct sockaddr_in *ipv4 =
            (const struct sockaddr_in *)address->ai_addr;
        uint32_t host_address = ntohl(ipv4->sin_addr.s_addr);
        return (host_address >> 24) != 127 && host_address != 0;
    }
    if (address->ai_family == AF_INET6) {
        const struct sockaddr_in6 *ipv6 =
            (const struct sockaddr_in6 *)address->ai_addr;
        if (IN6_IS_ADDR_V4MAPPED(&ipv6->sin6_addr)) {
            uint32_t v4_address;
            memcpy(&v4_address, &ipv6->sin6_addr.s6_addr[12],
                   sizeof(v4_address));
            v4_address = ntohl(v4_address);
            return (v4_address >> 24) != 127 && v4_address != 0;
        }
        return !IN6_IS_ADDR_LOOPBACK(&ipv6->sin6_addr) &&
               !IN6_IS_ADDR_UNSPECIFIED(&ipv6->sin6_addr);
    }
    return 0;
}

static int connect_remote(const char *host, const char *port) {
    if (!valid_remote_host(host)) {
        fprintf(stderr, "loopback/local endpoint is forbidden\n");
        return CONNECT_LOCAL_ADDRESS_REFUSED;
    }
    struct addrinfo hints = {0};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    struct addrinfo *addresses = NULL;
    int result = getaddrinfo(host, port, &hints, &addresses);
    if (result != 0) {
        fprintf(stderr, "getaddrinfo: %s\n", gai_strerror(result));
        return CONNECT_NAME_OR_SERVICE_ERROR;
    }
    int fd = -1;
    int rejected_local_address = 0;
    int setup_error = 0;
    int connect_attempted = 0;
    for (struct addrinfo *address = addresses; address != NULL;
         address = address->ai_next) {
        if (!address_is_remote(address)) {
            rejected_local_address = 1;
            continue;
        }
        fd = socket(address->ai_family, address->ai_socktype,
                    address->ai_protocol);
        if (fd < 0) {
            setup_error = 1;
            continue;
        }
        int flags = fcntl(fd, F_GETFL, 0);
        if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) {
            close(fd);
            fd = -1;
            setup_error = 1;
            continue;
        }
        int connected = connect(fd, address->ai_addr, address->ai_addrlen) == 0;
        if (!connected && errno == EINPROGRESS) {
            fd_set writable;
            FD_ZERO(&writable);
            FD_SET(fd, &writable);
            struct timeval timeout = {.tv_sec = 5, .tv_usec = 0};
            int ready = select(fd + 1, NULL, &writable, NULL, &timeout);
            int socket_error = 0;
            socklen_t error_size = sizeof(socket_error);
            if (ready == 0) {
                connect_attempted = 1;
            } else if (ready < 0 ||
                       getsockopt(fd, SOL_SOCKET, SO_ERROR, &socket_error,
                                  &error_size) != 0) {
                setup_error = 1;
            } else if (socket_error == 0) {
                connected = 1;
            } else {
                errno = socket_error;
                if (is_network_connect_error(socket_error)) {
                    connect_attempted = 1;
                } else {
                    setup_error = 1;
                }
            }
        } else if (!connected) {
            if (is_network_connect_error(errno)) {
                connect_attempted = 1;
            } else {
                setup_error = 1;
            }
        }
        if (connected) {
            if (fcntl(fd, F_SETFL, flags) != 0) {
                close(fd);
                fd = -1;
                setup_error = 1;
                continue;
            }
            struct timeval io_timeout = {.tv_sec = 5, .tv_usec = 0};
            if (setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &io_timeout,
                           sizeof(io_timeout)) != 0 ||
                setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &io_timeout,
                           sizeof(io_timeout)) != 0) {
                close(fd);
                fd = -1;
                setup_error = 1;
                continue;
            }
            break;
        }
        close(fd);
        fd = -1;
    }
    freeaddrinfo(addresses);
    if (fd >= 0) {
        return fd;
    }
    return classify_connect_outcome(setup_error, connect_attempted,
                                    rejected_local_address);
}

static int send_all(int fd, const unsigned char *data, size_t length) {
    size_t sent = 0;
    while (sent < length) {
        ssize_t count = send(fd, data + sent, length - sent, 0);
        if (count < 0 && errno == EINTR) {
            continue;
        }
        if (count <= 0) {
            return -1;
        }
        sent += (size_t)count;
    }
    return 0;
}

static int roundtrip(const char *host, const char *port, const char *nonce,
                     const char *input_path, const char *output_path) {
    FILE *input = fopen(input_path, "rb");
    if (input == NULL) {
        perror("open upload payload");
        return 1;
    }
    if (fseek(input, 0, SEEK_END) != 0) {
        perror("seek upload payload");
        fclose(input);
        return 1;
    }
    long length = ftell(input);
    if (length < 0 || length > 1024 * 1024 || fseek(input, 0, SEEK_SET) != 0) {
        fprintf(stderr, "payload size is invalid\n");
        fclose(input);
        return 1;
    }
    unsigned char *data = malloc((size_t)length + 1);
    if (data == NULL) {
        perror("allocate payload");
        fclose(input);
        return 1;
    }
    if (fread(data, 1, (size_t)length, input) != (size_t)length ||
        fclose(input) != 0) {
        fprintf(stderr, "read upload payload failed\n");
        free(data);
        return 1;
    }
    int fd = connect_remote(host, port);
    if (fd < 0) {
        fprintf(stderr, "connect upload endpoint failed\n");
        free(data);
        return 1;
    }
    if (send_all(fd, data, (size_t)length) != 0 || shutdown(fd, SHUT_WR) != 0) {
        perror("send upload payload");
        close(fd);
        free(data);
        return 1;
    }
    FILE *output = fopen(output_path, "wb");
    if (output == NULL) {
        perror("open downloaded payload");
        close(fd);
        free(data);
        return 1;
    }
    unsigned char reply[4096];
    size_t received = 0;
    for (;;) {
        ssize_t count = recv(fd, reply, sizeof(reply), 0);
        if (count < 0 && errno == EINTR) {
            continue;
        }
        if (count < 0) {
            perror("receive echoed payload");
            fclose(output);
            close(fd);
            free(data);
            return 1;
        }
        if (count == 0) {
            break;
        }
        if (received + (size_t)count > (size_t)length ||
            fwrite(reply, 1, (size_t)count, output) != (size_t)count) {
            fprintf(stderr, "unexpected response size or output error\n");
            fclose(output);
            close(fd);
            free(data);
            return 1;
        }
        received += (size_t)count;
    }
    int output_error = fclose(output) != 0;
    close(fd);
    free(data);
    if (output_error || received != (size_t)length) {
        fprintf(stderr, "short echo: sent=%ld received=%zu\n", length, received);
        return 1;
    }
    printf("OD_S5_F_UPLOAD nonce=%s bytes=%ld\n", nonce, length);
    printf("OD_S5_F_DOWNLOAD nonce=%s bytes=%zu\n", nonce, received);
    return 0;
}

static int expect_drop(const char *host, const char *port, const char *nonce) {
    int fd = connect_remote(host, port);
    if (fd < 0) {
        fprintf(stderr, "drop endpoint must accept then close the connection\n");
        return 1;
    }
    char request[160];
    int length = snprintf(request, sizeof(request), "drop-probe nonce=%s\n", nonce);
    if (length < 0 || (size_t)length >= sizeof(request)) {
        fprintf(stderr, "drop probe request formatting failed\n");
        close(fd);
        return 1;
    }
    if (send_all(fd, (const unsigned char *)request, (size_t)length) != 0) {
        int send_error = errno;
        close(fd);
        if (is_expected_drop_error(send_error)) {
            printf("OD_S5_F_DROP_ERROR nonce=%s observed=connection-reset-during-send\n", nonce);
            return 0;
        }
        errno = send_error;
        perror("send drop probe");
        return 1;
    }
    if (shutdown(fd, SHUT_WR) != 0) {
        int shutdown_error = errno;
        close(fd);
        if (is_expected_drop_error(shutdown_error)) {
            printf("OD_S5_F_DROP_ERROR nonce=%s observed=connection-reset-after-send\n", nonce);
            return 0;
        }
        errno = shutdown_error;
        perror("shutdown drop probe");
        return 1;
    }
    unsigned char response;
    ssize_t count;
    do {
        count = recv(fd, &response, 1, 0);
    } while (count < 0 && errno == EINTR);
    close(fd);
    int receive_error = errno;
    if (!is_expected_drop_response(count, receive_error)) {
        fprintf(stderr, "drop endpoint returned a response instead of closing\n");
        return 1;
    }
    printf("OD_S5_F_DROP_ERROR nonce=%s observed=connection-closed\n", nonce);
    return 0;
}

static int expect_unreachable(const char *host, const char *port,
                              const char *nonce) {
    int fd = connect_remote(host, port);
    if (fd == CONNECT_LOCAL_ADDRESS_REFUSED) {
        fprintf(stderr, "local/loopback endpoint is forbidden\n");
        return 1;
    }
    if (fd == CONNECT_NAME_OR_SERVICE_ERROR) {
        fprintf(stderr, "unavailable endpoint name/port did not resolve\n");
        return 1;
    }
    if (fd != CONNECT_NETWORK_FAILED) {
        if (fd >= 0) {
            close(fd);
        }
        fprintf(stderr, "unreachable probe did not fail specifically during connect\n");
        return 1;
    }
    printf("OD_S5_F_UNREACHABLE_ERROR nonce=%s observed=connect-failed\n", nonce);
    return 0;
}

int main(int argc, char **argv) {
    (void)signal(SIGPIPE, SIG_IGN);
    if (argc == 7 && strcmp(argv[1], "roundtrip") == 0) {
        return roundtrip(argv[2], argv[3], argv[4], argv[5], argv[6]);
    }
    if (argc == 5 && strcmp(argv[1], "expect-drop") == 0) {
        return expect_drop(argv[2], argv[3], argv[4]);
    }
    if (argc == 5 && strcmp(argv[1], "expect-unreachable") == 0) {
        return expect_unreachable(argv[2], argv[3], argv[4]);
    }
    fprintf(stderr, "invalid arguments\n");
    return 64;
}
