services:
#  db:
#    image: docker.io/library/postgres:15-alpine
#    restart: always
#    environment:
#      POSTGRES_USER: user
#      POSTGRES_PASSWORD: password
#      POSTGRES_DB: mydatabase
#    ports:
#      - "5432:5432"
#    volumes:
#      - postgres_data:/var/lib/postgresql/data

  postgres:
    image: docker.io/library/postgres:15-alpine
    container_name: postgres
    restart: unless-stopped

    environment:
      POSTGRES_DB: aida
      POSTGRES_USER_FILE: /run/secrets/pg_user
      POSTGRES_PASSWORD_FILE: /run/secrets/pg_password

    secrets:
      - pg_user
      - pg_password

    volumes:
      - pg_data:/var/lib/postgresql/data

    networks:
      - internal


  gitlab:
    image: gitlab/gitlab-ce:latest
    container_name: gitlab
    hostname: gitlab.local
    restart: unless-stopped
    shm_size: "256m"

    ports:
      - "80:80"     # HTTP
      - "443:443"   # HTTPS
      - "2222:22"   # SSH (map host 2222 -> container 22 to avoid clobbering host ssh)

    environment:
      GITLAB_OMNIBUS_CONFIG: |
        external_url 'http://gitlab.local'
        gitlab_rails['gitlab_shell_ssh_port'] = 2222

    volumes:
      - gitlab_config:/etc/gitlab
      - gitlab_logs:/var/log/gitlab
      - gitlab_data:/var/opt/gitlab

volumes:
  gitlab_config:
  gitlab_logs:
  gitlab_data:
  postgres_data:

secrets:
  pg_user:
    external: true
  pg_password:
    external: true
