FROM gradle:8.9.0-jdk8 AS build
COPY --chown=gradle:gradle . /home/gradle/src

WORKDIR /home/gradle/src

RUN chmod +x gradlew

RUN ./gradlew jar --no-daemon

FROM eclipse-temurin:17-jre-alpine

COPY --from=build /home/gradle/src/build/libs/*.jar /app/server.jar

ENTRYPOINT ["java","-Xmx384m","-jar", "/app/server.jar"]
