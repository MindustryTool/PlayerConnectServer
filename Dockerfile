FROM gradle:8.9.0-jdk22 AS build
COPY --chown=gradle:gradle . /home/gradle/src

WORKDIR /home/gradle/src

RUN gradlew jar --no-daemon 

FROM eclipse-temurin:8-jre-alpine

COPY --from=build /home/gradle/src/build/libs/*.jar /app/server.jar

ENTRYPOINT ["java","-jar", "/app/server.jar"]


