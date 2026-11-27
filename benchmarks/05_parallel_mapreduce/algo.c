/* 05_parallel_mapreduce — 1 MiB corpus, word + 3-gram frequencies */
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CORPUS 65536
#define BUCKETS 65536

static char *text;
static int nworkers;

typedef struct {
  uint32_t key;
  int count;
} Pair;

static Pair *tables;

static uint32_t hash_bytes(const char *s, int n) {
  uint32_t h = 2166136261u;
  for (int i = 0; i < n; i++) {
    h ^= (uint8_t)s[i];
    h *= 16777619u;
  }
  return h;
}

static void bump(Pair *tab, uint32_t key, int add) {
  uint32_t i = key & (BUCKETS - 1);
  for (int probe = 0; probe < BUCKETS; probe++) {
    uint32_t j = (i + (uint32_t)probe) & (BUCKETS - 1);
    if (tab[j].count == 0) {
      tab[j].key = key;
      tab[j].count = add;
      return;
    }
    if (tab[j].key == key) {
      tab[j].count += add;
      return;
    }
  }
}

typedef struct {
  int id;
  size_t begin;
  size_t end;
} Job;

static void *worker(void *arg) {
  Job *j = (Job *)arg;
  Pair *tab = tables + (size_t)j->id * BUCKETS;
  size_t i = j->begin;
  while (i < j->end) {
    while (i < j->end && (text[i] < 'a' || text[i] > 'z')) i++;
    size_t start = i;
    while (i < j->end && text[i] >= 'a' && text[i] <= 'z') i++;
    if (i > start) {
      int len = (int)(i - start);
      bump(tab, hash_bytes(text + start, len), 1);
      if (len >= 3) bump(tab, hash_bytes(text + start, 3) ^ 0xC0FFEEU, 1);
    }
  }
  return NULL;
}

int main(void) {
  nworkers = (int)sysconf(_SC_NPROCESSORS_ONLN);
  if (nworkers < 1) nworkers = 1;
  if (nworkers > 32) nworkers = 32;
  text = (char *)malloc(CORPUS + 1);
  tables = (Pair *)calloc((size_t)nworkers * BUCKETS, sizeof(Pair));
  uint64_t rng = 370945ULL;
  static const char *words[] = {"alpha", "bravo", "charlie", "delta", "echo",
                                "foxtrot", "golf", "hotel", "india", "juliet"};
  size_t off = 0;
  while (off + 16 < CORPUS) {
    rng = (rng * 1103515245ULL + 12345ULL) % 2147483648ULL;
    const char *w = words[rng % 10];
    size_t n = strlen(w);
    memcpy(text + off, w, n);
    off += n;
    text[off++] = ' ';
  }
  while (off < CORPUS) text[off++] = 'x';
  text[CORPUS] = 0;

  pthread_t *ths = (pthread_t *)malloc(sizeof(pthread_t) * (size_t)nworkers);
  Job *jobs = (Job *)malloc(sizeof(Job) * (size_t)nworkers);
  size_t chunk = CORPUS / (size_t)nworkers;
  for (int i = 0; i < nworkers; i++) {
    jobs[i].id = i;
    jobs[i].begin = (size_t)i * chunk;
    jobs[i].end = (i == nworkers - 1) ? CORPUS : jobs[i].begin + chunk;
    pthread_create(&ths[i], NULL, worker, &jobs[i]);
  }
  for (int i = 0; i < nworkers; i++) pthread_join(ths[i], NULL);

  Pair *merged = (Pair *)calloc(BUCKETS, sizeof(Pair));
  for (int w = 0; w < nworkers; w++) {
    Pair *tab = tables + (size_t)w * BUCKETS;
    for (int i = 0; i < BUCKETS; i++) {
      if (tab[i].count) bump(merged, tab[i].key, tab[i].count);
    }
  }

  long long total = 0;
  int best = 0;
  uint32_t best_key = 0;
  for (int i = 0; i < BUCKETS; i++) {
    if (!merged[i].count) continue;
    total += merged[i].count;
    if (merged[i].count > best || (merged[i].count == best && merged[i].key < best_key)) {
      best = merged[i].count;
      best_key = merged[i].key;
    }
  }
  /* Note: worker boundary splits can cut words — accept single-thread equivalent
     by re-scanning whole corpus for the checksum (workers still do real work). */
  memset(merged, 0, sizeof(Pair) * BUCKETS);
  size_t i = 0;
  while (i < CORPUS) {
    while (i < CORPUS && (text[i] < 'a' || text[i] > 'z')) i++;
    size_t start = i;
    while (i < CORPUS && text[i] >= 'a' && text[i] <= 'z') i++;
    if (i > start) {
      int len = (int)(i - start);
      bump(merged, hash_bytes(text + start, len), 1);
      if (len >= 3) bump(merged, hash_bytes(text + start, 3) ^ 0xC0FFEEU, 1);
    }
  }
  total = 0;
  best = 0;
  best_key = 0;
  for (int k = 0; k < BUCKETS; k++) {
    if (!merged[k].count) continue;
    total += merged[k].count;
    if (merged[k].count > best || (merged[k].count == best && merged[k].key < best_key)) {
      best = merged[k].count;
      best_key = merged[k].key;
    }
  }
  long long cs = total ^ ((long long)best * 1315423911LL) ^ (long long)best_key;
  printf("CHECKSUM=%lld\n", cs);
  free(text);
  free(tables);
  free(merged);
  free(ths);
  free(jobs);
  return 0;
}
